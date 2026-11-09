//! The ordinary WhatsApp turn's live tool block (#2609).
//!
//! [`super::flow_message`] owns the block itself (#2023): one living message,
//! a row per step, a 5 second tick, one settle. [`super::resume`] can open it
//! before the turn runs because a background completion is known to work. A
//! normal conversation turn is NOT known to, and a one-line reply must not
//! create and edit a message for nothing, so this wrapper opens the block on
//! the FIRST tool event and folds every later one into the same block.
//!
//! Two event kinds are deliberately left to their existing owners:
//! `IntermediateText` (already streamed by `stream.rs`) and `SuggestedOptions`
//! (`followups`, which also posts the held answer, #2030). Forwarding either
//! here would show the same content twice, so they never open a block.

use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;
use wacore_binary::jid::Jid;
use whatsapp_rust::client::Client;

use super::WhatsAppState;
use super::flow_message::{self, FlowConfig, FlowSender};
use super::flow_render::Phase;
use crate::brain::agent::ProgressEvent;

/// How long the settle waits for the block task to land its last edit before
/// the answer goes out anyway. The block is a progress view, never a gate on
/// delivery (#2030).
const SETTLE_TIMEOUT: Duration = Duration::from_secs(20);

/// Everything the block needs that is not the channel itself.
pub(crate) struct LiveBlockConfig {
    pub client: Arc<Client>,
    pub jid: Jid,
    pub state: Arc<WhatsAppState>,
    pub rate_limit: crate::config::WaRateLimitConfig,
    pub is_owner: bool,
    pub interactive_buttons: bool,
}

/// The spawned block: its write handle and the task that owns every send.
struct Live {
    sender: FlowSender,
    task: JoinHandle<()>,
}

/// Handle the turn holds. Cloned into the progress callback, settled once by
/// the turn before the answer is delivered.
#[derive(Clone)]
pub(crate) struct LiveBlock {
    /// `None` when config turned the block off, which keeps the plain-text
    /// path byte for byte what it was.
    config: Option<Arc<LiveBlockConfig>>,
    live: Arc<std::sync::Mutex<Option<Live>>>,
}

impl LiveBlock {
    pub(crate) fn new(config: LiveBlockConfig, enabled: bool) -> Self {
        Self {
            config: if enabled {
                Some(Arc::new(config))
            } else {
                None
            },
            live: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Whether this event means the turn is running tools. Narrow on purpose:
    /// these are the events `flow_render::fold` turns into a row.
    pub(crate) fn opens_block(event: &ProgressEvent) -> bool {
        matches!(
            event,
            ProgressEvent::ToolStarted { .. }
                | ProgressEvent::ToolCompleted { .. }
                | ProgressEvent::DetachedTaskStarted { .. }
                | ProgressEvent::DetachedTaskFinished { .. }
        )
    }

    /// Feed one progress event. Sync because the progress callback is sync and
    /// nothing here awaits: `spawn` and `forward` only hand work to the block
    /// task.
    pub(crate) fn observe(&self, event: &ProgressEvent) {
        let Some(config) = self.config.clone() else {
            return;
        };
        let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        if live.is_none() {
            if !Self::opens_block(event) {
                return;
            }
            let (sender, task) = flow_message::spawn(FlowConfig {
                client: config.client.clone(),
                jid: config.jid.clone(),
                state: config.state.clone(),
                rate_limit: config.rate_limit.clone(),
                is_owner: config.is_owner,
                interactive_buttons: config.interactive_buttons,
                // The normal turn never halts the block on options (that path
                // stays with `followups`), so nothing parks here.
                posted: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            });
            *live = Some(Live { sender, task });
        }
        if let Some(state) = live.as_ref() {
            state.sender.forward(event.clone());
        }
    }

    /// Settle the block, if one opened, BEFORE the answer is sent (#2030).
    pub(crate) async fn settle(&self, phase: Phase) {
        let live = {
            let mut slot = self.live.lock().unwrap_or_else(|e| e.into_inner());
            slot.take()
        };
        let Some(live) = live else {
            return;
        };
        live.sender.settle(phase);
        match tokio::time::timeout(SETTLE_TIMEOUT, live.task).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(
                target: "whatsapp",
                error = %e,
                "live tool block task panicked"
            ),
            Err(_) => tracing::warn!(
                target: "whatsapp",
                "live tool block still settling after {}s; sending the answer anyway",
                SETTLE_TIMEOUT.as_secs()
            ),
        }
    }

    /// Whether a block was opened for this turn. Test hook: the acceptance
    /// criteria turn on "no tool event, no block".
    #[cfg(test)]
    pub(crate) fn is_open(&self) -> bool {
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::LiveBlock;
    use crate::brain::agent::ProgressEvent;
    use serde_json::json;

    fn tool_started(name: &str) -> ProgressEvent {
        ProgressEvent::ToolStarted {
            tool_name: name.to_string(),
            tool_input: json!({}),
            tool_call_id: String::new(),
        }
    }

    #[test]
    fn tool_events_open_a_block() {
        assert!(LiveBlock::opens_block(&tool_started("bash")));
        assert!(LiveBlock::opens_block(&ProgressEvent::ToolCompleted {
            tool_name: "bash".to_string(),
            tool_input: json!({}),
            success: true,
            summary: "exit 0".to_string(),
            tool_call_id: String::new(),
        }));
        assert!(LiveBlock::opens_block(
            &ProgressEvent::DetachedTaskStarted {
                task_id: "t1".to_string(),
                label: "cargo test".to_string(),
            }
        ));
    }

    #[test]
    fn narration_and_options_do_not_open_a_block() {
        // Both already have an owner on this channel (#2609).
        assert!(!LiveBlock::opens_block(&ProgressEvent::Thinking));
        assert!(!LiveBlock::opens_block(&ProgressEvent::IntermediateText {
            text: "looking".to_string(),
            reasoning: None,
        }));
        assert!(!LiveBlock::opens_block(&ProgressEvent::SuggestedOptions(
            Vec::<crate::brain::tools::suggest_options::SuggestionItem>::new()
        )));
        assert!(!LiveBlock::opens_block(&ProgressEvent::TokenCount(12)));
    }

    #[test]
    fn a_disabled_block_never_opens() {
        // `None` config short-circuits before the slot is touched; the plain
        // text path stays exactly as it was.
        let block = LiveBlock {
            config: None,
            live: std::sync::Arc::new(std::sync::Mutex::new(None)),
        };
        block.observe(&tool_started("bash"));
        assert!(!block.is_open());
    }
}
