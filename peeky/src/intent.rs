//! Exact-match keyword allowlist. A drift guard, not a classifier.
//!
//! Returns `Some(Intent)` only when the whole utterance IS one of a few
//! high-frequency transport commands ("play", "skip", "next track"). Anything
//! with extra words ("play sicko mode", "skip to the chorus") returns `None`
//! and falls through to the on-device routelet classifier in the orchestrator.
//!
//! Why exact-match only, and why so short: routelet now generalizes across
//! phrasings, so the old prefix/substring keyword classifier mostly masked it,
//! and its substring rules (e.g. matching " the button" anywhere) fired on
//! inputs that are routelet's job. The one thing a hand-written layer still
//! earns is a deterministic floor for the commands said dozens of times a day,
//! where a model boundary shifting across a retrain is unacceptable. Embedding
//! a transport word in a sentence is NOT covered on purpose: "play sicko mode"
//! must reach routelet, not short-circuit here.

use crate::providers::claude::Intent;

/// Sub-microsecond exact-match check. The trimmed, lowercased transcript (with
/// trailing sentence punctuation removed) must equal an allowlisted transport
/// command. Returns `None` for everything else, which the orchestrator routes
/// to routelet next.
pub fn keyword_classify(transcript: &str) -> Option<Intent> {
    let lower = transcript.trim().to_lowercase();
    let normalized = lower.trim_end_matches(['.', '!', '?']).trim_end();
    match normalized {
        "play" | "pause" | "resume" | "stop" | "mute" | "unmute" | "skip" | "next"
        | "next song" | "next track" | "previous" | "previous song" | "previous track" => {
            Some(Intent::Integration)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_transport_commands_match() {
        for cmd in [
            "play",
            "pause",
            "resume",
            "stop",
            "mute",
            "unmute",
            "skip",
            "next",
            "next song",
            "next track",
            "previous",
            "previous song",
            "previous track",
        ] {
            assert_eq!(
                keyword_classify(cmd),
                Some(Intent::Integration),
                "{cmd} should match the allowlist"
            );
        }
    }

    #[test]
    fn case_and_trailing_punctuation_normalized() {
        assert_eq!(keyword_classify("  Play. "), Some(Intent::Integration));
        assert_eq!(keyword_classify("SKIP!"), Some(Intent::Integration));
        assert_eq!(keyword_classify("Next Track?"), Some(Intent::Integration));
    }

    #[test]
    fn transport_word_inside_a_sentence_falls_through() {
        // The whole point of exact-match: a transport word with any extra words
        // must reach routelet, never short-circuit here.
        assert_eq!(keyword_classify("play sicko mode"), None);
        assert_eq!(keyword_classify("skip to the chorus"), None);
        assert_eq!(keyword_classify("can you pause this"), None);
        assert_eq!(keyword_classify("play something on spotify"), None);
    }

    #[test]
    fn non_transport_falls_through_to_routelet() {
        assert_eq!(keyword_classify("where is the search bar"), None);
        assert_eq!(keyword_classify("check my gmail"), None);
        assert_eq!(keyword_classify("remember my name is Daniel"), None);
        assert_eq!(keyword_classify("what's my favorite color"), None);
        assert_eq!(keyword_classify("hello"), None);
        assert_eq!(keyword_classify("click the play button"), None);
        assert_eq!(keyword_classify(""), None);
    }
}
