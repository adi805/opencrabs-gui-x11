//! `provider/model` pair parsing, shared by every non-interactive switch
//! surface (#461 family: CLI set-model, direct-arg /models on channels and
//! the TUI). The FIRST slash splits, so model names that contain slashes
//! survive intact (`openrouter/tencent/hy3:free` → `openrouter` +
//! `tencent/hy3:free`).

pub fn parse_pair(pair: &str) -> Result<(String, String), String> {
    match pair.split_once('/') {
        Some((provider, model)) if !provider.trim().is_empty() && !model.trim().is_empty() => {
            Ok((provider.trim().to_string(), model.trim().to_string()))
        }
        _ => Err(format!(
            "'{pair}' is not a provider/model pair — expected e.g. \
             openrouter/tencent/hy3:free (the first slash splits)"
        )),
    }
}

/// Resolve `model` to the spelling the provider's own catalogue uses.
///
/// Some catalogues list bare ids (`deepseek-v4.1-flash`); others include the
/// provider itself because that is the id the endpoint expects on the wire
/// (`ngodeai/glm-5.3-flash`). A session pin and a `/models` argument can
/// carry EITHER spelling, and comparing those raw strings made every bare
/// pin look foreign to a prefixed catalogue: the stale-pin guard then
/// replaced a perfectly servable pick with the provider default, and every
/// banner repeated the provider name (`ngodeai/ngodeai/glm-5.3-flash`).
///
/// Returns the catalogue spelling when either form matches, the input
/// unchanged when the catalogue is empty (nothing to validate against), and
/// `None` when the catalogue is non-empty and NEITHER form is listed — the
/// caller decides whether to substitute a default (stale-pin guard) or
/// reject the switch (unknown model).
pub fn canonical_model(provider: &str, model: &str, supported: &[String]) -> Option<String> {
    if supported.is_empty() {
        return Some(model.to_string());
    }
    if supported.iter().any(|m| m == model) {
        return Some(model.to_string());
    }
    // Bare pin against a catalogue that stores the provider prefix.
    let prefixed = format!("{provider}/{model}");
    if supported.iter().any(|m| m == &prefixed) {
        return Some(prefixed);
    }
    // Prefixed pin against a catalogue that stores bare ids.
    let prefix = format!("{provider}/");
    if let Some(bare) = model.strip_prefix(&prefix) {
        if !bare.is_empty() && supported.iter().any(|m| m == bare) {
            return Some(bare.to_string());
        }
    }
    None
}

/// Compose `provider/model` for a USER-FACING banner without repeating the
/// provider name when the model id already carries it. Display only: the
/// request still ships the catalogue spelling untouched.
pub fn display_model(provider: &str, model: &str) -> String {
    let prefix = format!("{provider}/");
    match model.strip_prefix(&prefix) {
        Some(rest) if !rest.is_empty() => format!("{provider}/{rest}"),
        _ => format!("{provider}/{model}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat(models: &[&str]) -> Vec<String> {
        models.iter().map(|m| m.to_string()).collect()
    }

    #[test]
    fn bare_pin_resolves_to_prefixed_catalogue_entry() {
        // The 2026-10-31 regression: ngodeai's catalogue stores
        // `ngodeai/glm-5.3-flash-fast`, a `/models` switch pinned the bare
        // `glm-5.3-flash-fast`, and the guard dropped the user's pick.
        let supported = cat(&["ngodeai/glm-5.3-flash", "ngodeai/glm-5.3-flash-fast"]);
        assert_eq!(
            canonical_model("ngodeai", "glm-5.3-flash-fast", &supported).as_deref(),
            Some("ngodeai/glm-5.3-flash-fast")
        );
    }

    #[test]
    fn prefixed_pin_resolves_to_bare_catalogue_entry() {
        let supported = cat(&["glm-5.3-flash", "glm-5.3-flash-fast"]);
        assert_eq!(
            canonical_model("ngodeai", "ngodeai/glm-5.3-flash-fast", &supported).as_deref(),
            Some("glm-5.3-flash-fast")
        );
    }

    #[test]
    fn exact_match_wins_and_foreign_model_returns_none() {
        let supported = cat(&["glm-5.3-flash"]);
        assert_eq!(
            canonical_model("ngodeai", "glm-5.3-flash", &supported).as_deref(),
            Some("glm-5.3-flash")
        );
        assert_eq!(
            canonical_model("ngodeai", "gemini-3.8-flash", &supported),
            None
        );
    }

    #[test]
    fn empty_catalogue_accepts_pin_unchanged() {
        assert_eq!(
            canonical_model("minimal", "whatever", &[]).as_deref(),
            Some("whatever")
        );
    }

    #[test]
    fn display_model_never_repeats_the_provider() {
        assert_eq!(
            display_model("ngodeai", "ngodeai/glm-5.3-flash"),
            "ngodeai/glm-5.3-flash"
        );
        // A sub-vendor prefix (inferhub routes alicn/..., ag/...) is part of
        // the id and must stay visible.
        assert_eq!(
            display_model("inferhub", "alicn/deepseek-v4.1-flash"),
            "inferhub/alicn/deepseek-v4.1-flash"
        );
        assert_eq!(display_model("zai", "glm-5.1"), "zai/glm-5.1");
    }
}
