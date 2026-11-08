//! Pins the cross-provider model leak guard at
//! `tool_loop::guard_cross_provider_model_leak`.
//!
//! Regression: 2026-06-04. Session `fd72101f` ran on `dialagram` earlier in
//! the day. A sticky fallback fired and `persist_sticky_pair` wrote
//! `qwen-3.7-max-thinking` into `session.model` in DB AND into the in-memory
//! `session_models[sid]` override. The user later set up a new
//! `modelscope-qwen` custom provider via the `/models` dialog. On the next
//! turn, `tool_loop` resolved `request.model` from the stale session pin and
//! shipped `qwen-3.7-max-thinking` to modelscope-qwen, which 400'd with
//! "Invalid model id". The fallback then re-pinned `zhipu / glm-5.1` onto
//! the session row, persisting yet another cross-provider mash-up.
//!
//! The fix has three legs:
//!   1. `persist_sticky_pair` is now a no-op for persistence (fallback is
//!      per-request, never per-session).
//!   2. The `/models` setup path persists what the user explicitly picked
//!      (DB row + in-memory override) on confirmation.
//!   3. **This file** pins the request-time guard: if a model pinned for a
//!      session isn't in the active provider's catalogue, the guard
//!      substitutes the provider's own default so no request ever ships
//!      with a model name from a different provider.
//!
//! These three layers compose: even if 1 and 2 regress in a future refactor,
//! 3 stops the cross-provider request from going out.

use crate::brain::agent::service::tool_loop::guard_cross_provider_model_leak;

#[test]
fn stale_pin_from_other_provider_is_substituted_with_active_default() {
    // modelscope-qwen catalogue has Qwen-Ambassador/Qwen3.7-Max but not
    // qwen-3.7-max-thinking (that was a dialagram model). The pin must be
    // dropped in favour of the active provider's default.
    let supported = vec![
        "Qwen-Ambassador/Qwen3.7-Max".to_string(),
        "Qwen-Ambassador/Qwen3.5-Plus".to_string(),
    ];
    let (chosen, leaked) = guard_cross_provider_model_leak(
        "qwen-3.7-max-thinking".to_string(),
        "modelscope-qwen",
        "Qwen-Ambassador/Qwen3.7-Max",
        &supported,
    );
    assert_eq!(chosen, "Qwen-Ambassador/Qwen3.7-Max");
    assert_eq!(
        leaked.as_deref(),
        Some("qwen-3.7-max-thinking"),
        "the substitution must surface the stale name so the caller can log it; \
         the user needs to see WHICH model leaked, not just that one did"
    );
}

#[test]
fn pinned_model_in_active_catalogue_passes_through_untouched() {
    // Happy path: user picked Qwen-Ambassador/Qwen3.7-Max on modelscope-qwen
    // and the next turn finds it in the supported list. No substitution.
    let supported = vec!["Qwen-Ambassador/Qwen3.7-Max".to_string()];
    let (chosen, leaked) = guard_cross_provider_model_leak(
        "Qwen-Ambassador/Qwen3.7-Max".to_string(),
        "modelscope-qwen",
        "Qwen-Ambassador/Qwen3.7-Max",
        &supported,
    );
    assert_eq!(chosen, "Qwen-Ambassador/Qwen3.7-Max");
    assert!(
        leaked.is_none(),
        "in-catalogue pin must not be flagged as a leak"
    );
}

#[test]
fn empty_catalogue_accepts_any_pin() {
    // Providers without a `/v1/models` impl (or with no `models = [...]`
    // declared in config.toml) return an empty supported_models list. We
    // can't tell the pin is wrong, so we have to trust it — the alternative
    // is breaking every minimal custom provider whose catalogue we can't
    // introspect. Document this carve-out via test so it can't quietly
    // change.
    let supported: Vec<String> = vec![];
    let (chosen, leaked) = guard_cross_provider_model_leak(
        "any-model-name-the-user-typed".to_string(),
        "minimal",
        "provider-default",
        &supported,
    );
    assert_eq!(chosen, "any-model-name-the-user-typed");
    assert!(leaked.is_none());
}

#[test]
fn substitution_uses_active_provider_default_not_the_pin() {
    // Verify the substituted value is exactly `provider_default`, not the
    // first entry in `supported` or anything else. A future refactor that
    // "picks the first supported model" would still pass the basic
    // substitute-on-miss test but break the user's intent (the active
    // provider may have a curated default that isn't supported[0]).
    let supported = vec!["list-first-model".to_string(), "active-default".to_string()];
    let (chosen, leaked) = guard_cross_provider_model_leak(
        "stale-pin".to_string(),
        "modelscope-qwen",
        "active-default",
        &supported,
    );
    assert_eq!(chosen, "active-default");
    assert_eq!(leaked.as_deref(), Some("stale-pin"));
}

#[test]
fn case_sensitive_match_no_partial_substring_pass_through() {
    // The catalogue check is exact-equal, not case-insensitive or substring.
    // A provider that lists `Qwen-Ambassador/Qwen3.7-Max` does NOT accept a
    // pin of `qwen-ambassador/qwen3.7-max` — they're different model ids on
    // some routers (notably ModelScope where casing matters in the path).
    let supported = vec!["Qwen-Ambassador/Qwen3.7-Max".to_string()];
    let (chosen, leaked) = guard_cross_provider_model_leak(
        "qwen-ambassador/qwen3.7-max".to_string(),
        "modelscope-qwen",
        "Qwen-Ambassador/Qwen3.7-Max",
        &supported,
    );
    assert_eq!(
        chosen, "Qwen-Ambassador/Qwen3.7-Max",
        "lowercased pin must be substituted, not silently coerced"
    );
    assert!(leaked.is_some());
}

// ── 2026-10-31 regression: prefix-tolerant canonicalisation ────────
//
// ngodeai is the ONLY custom provider whose catalogue stores the
// provider-qualified wire id (`models = ["ngodeai/...", ...]`), while every
// other provider stores bare ids. Two failures fell out of that:
//   1. A `/models ngodeai/glm-5.3-flash-fast` switch persisted the BARE
//      `glm-5.3-flash-fast` in some surfaces; the old guard saw a bare pin
//      against a prefixed catalogue and silently substituted the provider
//      default EVERY TURN — the user's pick never survived.
//   2. Banners composed `provider/model` over a model id that already
//      carried the prefix → `ngodeai/ngodeai/glm-5.3-flash` (204 log hits
//      on 2026-10-31 alone).
// The guard now resolves EITHER spelling to the catalogue's own via
// `provider_pair::canonical_model` before declaring anything stale.

#[test]
fn bare_pin_against_prefixed_catalogue_resolves_not_substituted() {
    // `glm-5.3-flash-fast` IS a real ngodeai model, just stored prefixed.
    let supported = vec!["ngodeai/glm-5.3-flash".to_string()];
    let (chosen, leaked) = guard_cross_provider_model_leak(
        "glm-5.3-flash".to_string(),
        "ngodeai",
        "ngodeai/glm-5.3-flash",
        &supported,
    );
    assert_eq!(
        chosen, "ngodeai/glm-5.3-flash",
        "a bare pin for a prefixed-catalogue model must resolve to the catalogue spelling, \
         not be substituted with the default"
    );
    assert!(leaked.is_none());
}

#[test]
fn prefixed_pin_against_bare_catalogue_resolves_not_substituted() {
    // Symmetric case: catalogue stores bare ids, pin carries the provider
    // prefix (e.g. restored from a display string).
    let supported = vec![
        "glm-5.3-flash".to_string(),
        "glm-5.3-flash-fast".to_string(),
    ];
    let (chosen, leaked) = guard_cross_provider_model_leak(
        "ngodeai/glm-5.3-flash-fast".to_string(),
        "ngodeai",
        "glm-5.3-flash",
        &supported,
    );
    assert_eq!(chosen, "glm-5.3-flash-fast");
    assert!(leaked.is_none());
}

#[test]
fn self_prefixed_pin_in_prefixed_catalogue_passes_through() {
    // Double-prefix-shaped pin `ngodeai/ngodeai/glm-5.3-flash`: stripping the
    // provider prefix leaves `ngodeai/glm-5.3-flash`, which IS the catalogue
    // spelling — accept it as-is so a display artifact never breaks routing.
    let supported = vec!["ngodeai/glm-5.3-flash".to_string()];
    let (chosen, leaked) = guard_cross_provider_model_leak(
        "ngodeai/ngodeai/glm-5.3-flash".to_string(),
        "ngodeai",
        "ngodeai/glm-5.3-flash",
        &supported,
    );
    assert_eq!(chosen, "ngodeai/ngodeai/glm-5.3-flash");
    assert!(leaked.is_none());
}

// ── Source-level invariant guard ──────────────────────────────────
//
// persist_sticky_pair MUST stay a persistence no-op. The function exists
// only for compatibility with the dozen+ call sites in tool_loop.rs; its
// body must never write to `session_models`, `set_session_model`, or call
// `update_session`. A future "let's restore the persist for display"
// refactor would re-introduce the 2026-06-04 cross-provider mash-up.

const BUILDER_SRC: &str = include_str!("../brain/agent/service/builder.rs");

#[test]
fn persist_sticky_pair_does_not_write_session_state() {
    // Locate the function body. Tolerate doc-comments above the signature
    // by anchoring on the function signature itself.
    let sig = "pub(crate) fn persist_sticky_pair(";
    let start = BUILDER_SRC
        .find(sig)
        .expect("persist_sticky_pair function must exist in builder.rs");
    // Body ends at the matching closing brace at the top level of the impl
    // block. Heuristic: scan forward to the first `\n    }\n` — works
    // because the function lives at indentation depth 1 inside the impl.
    let rest = &BUILDER_SRC[start..];
    let end_off = rest
        .find("\n    }\n")
        .expect("could not bound persist_sticky_pair function body");
    let body = &rest[..end_off];

    for forbidden in [
        "set_session_model",
        "session_models",
        "update_session",
        "spawn",
    ] {
        assert!(
            !body.contains(forbidden),
            "persist_sticky_pair body contains forbidden token `{forbidden}`. \
             This function MUST stay a no-op for persistence — a transient \
             rescue can't be allowed to mutate the user's session pick. \
             See tool_loop::guard_cross_provider_model_leak doc + the \
             2026-06-04 fd72101f incident notes."
        );
    }
}
