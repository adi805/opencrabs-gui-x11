//! Bounded, retried HTTP downloads for channel media (#2261).
//!
//! Every inbound media path used to be a one-shot `reqwest::get` with no
//! timeout: a DNS blip, a connection reset, a TLS hiccup or a stalled socket
//! ended the message permanently, the user was told the download failed, and
//! the file never reached the agent. That is the same failure class #1785
//! fixed for startup `get_me()`; this helper brings the same treatment to the
//! download sites.
//!
//! Contract: a bounded number of attempts, exponential backoff with jitter,
//! a per-attempt timeout, and a status check BEFORE the body is read. Server
//! errors, 408 and 429 are retried; a 4xx that will not change on a retry is
//! returned immediately.

use std::time::Duration;

use crate::utils::retry::{RetryConfig, retry_with_check};

/// Ceiling for one attempt: enough for a 20 MB Telegram document on a slow
/// link, short enough that a stalled socket cannot hang a chat turn forever.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(90);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A download failure that knows whether another attempt could help.
#[derive(Debug)]
pub struct DownloadError {
    message: String,
    retryable: bool,
}

impl DownloadError {
    fn fatal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }

    fn transient(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DownloadError {}

/// Download `url` into memory, retrying transient failures.
pub async fn fetch_bytes(url: &str) -> Result<Vec<u8>, DownloadError> {
    let client = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|e| DownloadError::fatal(format!("http client build failed: {e}")))?;

    let target = url.to_string();
    retry_with_check(
        || {
            let client = client.clone();
            let target = target.clone();
            async move { fetch_once(&client, &target).await }
        },
        &RetryConfig::default(),
        |e| e.retryable,
    )
    .await
}

async fn fetch_once(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, DownloadError> {
    let resp = client.get(url).send().await.map_err(|e| {
        if e.is_builder() {
            DownloadError::fatal(format!("request could not be built: {e}"))
        } else {
            // Timeout, connect error, reset, TLS hiccup: worth another try.
            DownloadError::transient(format!("request failed: {e}"))
        }
    })?;

    let status = resp.status();
    if !status.is_success() {
        let retryable =
            status.is_server_error() || status.as_u16() == 408 || status.as_u16() == 429;
        let message = format!("HTTP {status}");
        return Err(if retryable {
            DownloadError::transient(message)
        } else {
            DownloadError::fatal(message)
        });
    }

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| DownloadError::transient(format!("reading body failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Serve `responses` in order on a real socket, one connection each.
    fn serve(responses: Vec<(u16, &'static str)>) -> (String, std::thread::JoinHandle<usize>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let handle = std::thread::spawn(move || {
            let want = responses.len();
            let mut served = 0;
            // A terminal status (404) makes the client stop after one attempt,
            // so the remaining scripted responses never arrive. A blocking
            // accept would then hang `join()` forever instead of failing an
            // assertion, so cap the wait.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while served < want && std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut buf = [0u8; 1024];
                        let _ = stream.read(&mut buf);
                        let (status, body) = responses[served];
                        let reply = format!(
                            "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(reply.as_bytes());
                        served += 1;
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
            served
        });
        (format!("http://{addr}/file"), handle)
    }

    #[tokio::test]
    async fn retries_a_transient_server_error_then_succeeds() {
        let (url, server) = serve(vec![(503, "busy"), (200, "payload")]);
        let bytes = fetch_bytes(&url).await.expect("second attempt succeeds");
        assert_eq!(bytes, b"payload");
        assert_eq!(server.join().expect("server"), 2, "both attempts served");
    }

    #[tokio::test]
    async fn does_not_retry_a_terminal_client_error() {
        let (url, server) = serve(vec![(404, "gone"), (200, "should not be used")]);
        let err = fetch_bytes(&url).await.expect_err("404 must fail");
        assert!(err.to_string().contains("404"), "got {err}");
        assert_eq!(
            server.join().expect("server"),
            1,
            "a 404 must be attempted exactly once"
        );
    }
}
