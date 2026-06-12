//! PII and secret redaction for transcripts. Applied identically at training
//! and inference so there is no train/serve skew, and again before any sample
//! is logged or uploaded. Over-redaction is acceptable; under-redaction is not.

use std::sync::OnceLock;

use regex::Regex;

use crate::providers::claude::Intent;

struct Redactors {
    /// Matches assignment-cue phrases and captures the value that follows.
    /// Used only for Memory intent to mask stored values like "my name is Daniel".
    memory_assign: Regex,
    /// After a secret keyword (password, token, etc.), masks the remainder of
    /// the phrase to prevent credential leakage in any intent.
    secret_keyword: Regex,
    /// Email addresses.
    email: Regex,
    /// Runs of 4 or more digits (card numbers, PINs, phone numbers, etc.).
    /// Short numbers like "50" or "3" are intentionally left alone.
    digit_run: Regex,
}

static REDACTORS: OnceLock<Redactors> = OnceLock::new();

fn redactors() -> &'static Redactors {
    REDACTORS.get_or_init(|| Redactors {
        // reason: every pattern is a static literal. A malformed one would fail
        // deterministically on first use and the unit tests would catch it, so
        // these unwraps cannot fire at runtime.
        // Keep everything up to and including the cue word; replace trailing value.
        memory_assign: Regex::new(r"(?i)\b(is|are|=|equals)\b\s+\S.*$").unwrap(),
        // Keep the keyword itself; blank out everything that follows.
        secret_keyword: Regex::new(
            r"(?i)\b(password|passcode|pin|ssn|secret|token|api\s*key|api\s*secret|credit card|card number)\b.*$",
        )
        .unwrap(),
        email: Regex::new(r"(?i)[a-z0-9._%+\-]+@[a-z0-9.\-]+\.[a-z]{2,}").unwrap(),
        digit_run: Regex::new(r"\b\d{4,}\b").unwrap(),
    })
}

/// Intent-independent normalization applied identically at training and
/// inference. Masks secrets, emails, and long digit runs. Does NOT apply the
/// memory assign-cue rule, which is intent-dependent and storage-only.
///
/// Rules in order (earlier rules can consume text that later rules never see):
///   1. Lowercase, to match the all-lowercase training corpus.
///   2. Strip trailing whitespace and terminal `.!?` that STT appends, but if
///      that trimmed tail held a `?`, append exactly one `?` back at the end.
///      STT periods are noise; a terminal question mark is signal for the
///      question register (capability checks, tag questions) the model is
///      trained on via the augmenter's `?` variants.
///   3. Secret keyword tail: keep the keyword, mask everything after it.
///   4. Email addresses -> `<EMAIL>`.
///   5. Runs of 4+ digits -> `<NUM>`.
pub(super) fn preprocess(text: &str) -> String {
    let r = redactors();

    // Rules 1-2: lowercase, then trim trailing whitespace and terminal `.!?`,
    // remembering whether the trimmed tail held a question mark. The training
    // corpus is all lowercase, so this closes the surface skew.
    let lowered = text.to_lowercase();
    let normalized =
        lowered.trim_end_matches(|c: char| matches!(c, '.' | '!' | '?') || c.is_whitespace());
    let is_question = lowered[normalized.len()..].contains('?');

    // Rule 3: "password is hunter2" -> "password <SECRET>".
    let s = r
        .secret_keyword
        .replace(normalized, |caps: &regex::Captures| {
            format!("{} <SECRET>", &caps[1])
        })
        .into_owned();

    // Rule 2: emails.
    let s = r.email.replace_all(&s, "<EMAIL>").into_owned();

    // Rule 3: runs of 4+ digits (PINs, card numbers, phone numbers, etc.).
    let s = r.digit_run.replace_all(&s, "<NUM>").into_owned();

    // Rule 2's question tail, restored after masking so it stays terminal.
    if is_question { s + "?" } else { s }
}

/// Redact PII and secrets from a voice transcript before writing to the log.
/// Applies `preprocess` for all intents, then the memory assign-cue rule for
/// Memory turns. Over-redaction is acceptable; under-redaction is not.
pub(super) fn redact(text: &str, intent: Intent) -> String {
    let s = preprocess(text);

    // Memory assign-cue (Memory intent only). "my name is Daniel" ->
    // "my name is <SECRET>". Applied after preprocess so credentials already
    // masked there are not re-processed.
    if intent == Intent::Memory {
        redactors()
            .memory_assign
            .replace(&s, |caps: &regex::Captures| {
                format!("{} <SECRET>", &caps[1])
            })
            .into_owned()
    } else {
        s
    }
}

/// Pick the intent to redact for. The two classifiers can disagree, so redact
/// for the most sensitive interpretation: if either thinks this is a Memory
/// turn, apply the memory assign-cue masking so a stored secret never lands in
/// the sample. Otherwise prefer the Claude label (the teacher), then routelet,
/// then default to Chat (the least-redacting branch, which is safe for the
/// generic preprocess pass that still runs).
pub(super) fn redact_intent(routelet_pred: Option<Intent>, claude_label: Option<Intent>) -> Intent {
    if routelet_pred == Some(Intent::Memory) || claude_label == Some(Intent::Memory) {
        return Intent::Memory;
    }
    claude_label.or(routelet_pred).unwrap_or(Intent::Chat)
}

#[cfg(test)]
mod tests {
    use super::*;

    // redact: benign commands are untouched
    #[test]
    fn redact_benign_integration_unchanged() {
        let out = redact("play despacito on spotify", Intent::Integration);
        assert_eq!(out, "play despacito on spotify");
    }

    #[test]
    fn redact_benign_chat_unchanged() {
        let out = redact("what's the capital of france", Intent::Chat);
        assert_eq!(out, "what's the capital of france");
    }

    // redact_intent: Memory wins whenever either classifier picks it, so the
    // assign-cue masking still fires when the two labels disagree.
    #[test]
    fn redact_intent_memory_wins_on_disagreement() {
        // routelet guessed Chat, Claude corrected to Memory.
        assert_eq!(
            redact_intent(Some(Intent::Chat), Some(Intent::Memory)),
            Intent::Memory,
        );
        // routelet guessed Memory, Claude didn't run.
        assert_eq!(redact_intent(Some(Intent::Memory), None), Intent::Memory);
        // Neither is Memory: prefer the Claude label.
        assert_eq!(
            redact_intent(Some(Intent::Chat), Some(Intent::Integration)),
            Intent::Integration,
        );
        // No labels at all: default to Chat.
        assert_eq!(redact_intent(None, None), Intent::Chat);
    }

    // redact: secret keyword masks trailing value
    #[test]
    fn redact_wifi_password() {
        let out = redact("my wifi password is hunter2", Intent::Memory);
        assert!(out.contains("<SECRET>"), "expected <SECRET> in: {out}");
        assert!(
            !out.contains("hunter2"),
            "hunter2 should be masked in: {out}"
        );
    }

    // redact: memory assign-cue masks stored value
    #[test]
    fn redact_remember_name() {
        let out = redact("remember my name is daniel", Intent::Memory);
        assert!(out.contains("<SECRET>"), "expected <SECRET> in: {out}");
        assert!(
            !out.contains("daniel"),
            "name value should be masked in: {out}"
        );
    }

    // redact: assign-cue does NOT fire for non-Memory intents
    #[test]
    fn redact_assign_cue_chat_not_masked() {
        // "the sky is blue" in a Chat turn should keep "blue"
        let out = redact("the sky is blue", Intent::Chat);
        assert!(
            out.contains("blue"),
            "non-memory assign cue should be left alone: {out}"
        );
    }

    // redact: email masking
    #[test]
    fn redact_email() {
        let out = redact("email me at a@b.com", Intent::Chat);
        assert!(out.contains("<EMAIL>"), "expected <EMAIL> in: {out}");
        assert!(!out.contains("a@b.com"), "email should be masked in: {out}");
    }

    // redact: 4+ digit runs masked
    #[test]
    fn redact_long_digit_run_memory() {
        let out = redact("my code is 904112", Intent::Memory);
        // The memory assign-cue rule fires first and masks "904112" as <SECRET>;
        // the digit rule does not need to fire for the value to be gone.
        assert!(
            !out.contains("904112"),
            "6-digit code should be masked in: {out}"
        );
        assert!(
            out.contains("<SECRET>") || out.contains("<NUM>"),
            "expected a mask token in: {out}"
        );
    }

    #[test]
    fn redact_long_digit_run_integration() {
        let out = redact("call 5551234", Intent::Integration);
        assert!(
            !out.contains("5551234"),
            "phone number should be masked in: {out}"
        );
        assert!(out.contains("<NUM>"), "expected <NUM> in: {out}");
    }

    // redact: small numbers (1-3 digits) are preserved
    #[test]
    fn redact_small_number_preserved() {
        let out = redact("set volume to 50", Intent::Integration);
        assert!(out.contains("50"), "two-digit number should survive: {out}");
    }

    // preprocess: shared fixture conformance. Every vector in
    // tests/preprocess_vectors.json must match exactly.
    #[test]
    fn preprocess_conformance() {
        let raw = include_str!("../../tests/preprocess_vectors.json");
        let fixture: serde_json::Value =
            serde_json::from_str(raw).expect("preprocess_vectors.json must parse");
        let vectors = fixture["vectors"]
            .as_array()
            .expect("'vectors' must be an array");
        for v in vectors {
            let input = v["in"].as_str().expect("vector 'in' must be a string");
            let want = v["out"].as_str().expect("vector 'out' must be a string");
            let got = preprocess(input);
            assert_eq!(got, want, "preprocess({input:?}) = {got:?}, want {want:?}");
        }
    }
}
