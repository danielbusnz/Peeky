//! Routing regression suite. Runs real transcripts through the real
//! on-device routing stack (agent cue, keyword allowlist, routelet with the
//! shipped ONNX model) and asserts the one invariant that matters:
//!
//!   routelet may be unsure (defer to the LLM classifier), but it must
//!   never be confidently WRONG.
//!
//! A confident misroute sails past the fallback threshold and lands the
//! turn on a path with the wrong toolbox ("type X here" classified as
//! Integration cannot type; it improvises). Deferring is always a pass
//! here: the LLM tie-breaker exists for exactly that case and is not free,
//! so it is not exercised in tests.
//!
//! The golden set is seeded from real session transcripts (2026-06-12,
//! the night the lease demo was filmed). When a routelet retrain shifts a
//! boundary, this suite says whether the shift broke a known-good phrase.
//! Known misroutes that the next retrain must fix live in the #[ignore]
//! tests at the bottom; run them with `cargo test -- --ignored`.

use std::path::PathBuf;
use std::sync::OnceLock;

use peeky::agent_cue::agent_cue;
use peeky::intent::keyword_classify;
use peeky::providers::claude::Intent;
use peeky::routelet::Routelet;
use peeky::tuning::ROUTELET_CONFIDENCE_THRESHOLD;

/// The shipped model, loaded once for the whole test binary.
fn routelet() -> &'static Routelet {
    static MODEL: OnceLock<Routelet> = OnceLock::new();
    MODEL.get_or_init(|| {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../models/routelet");
        Routelet::load(&dir).expect("load shipped routelet model from models/routelet")
    })
}

/// Pass when routelet defers (low confidence or the reject class) or
/// confidently picks an accepted intent. Fail only on a confident intent
/// outside `accepted`: that is the misroute class of bug.
fn assert_no_confident_misroute(phrase: &str, accepted: &[Intent]) {
    match routelet().classify_with_confidence(phrase) {
        Some((intent, conf)) if conf >= ROUTELET_CONFIDENCE_THRESHOLD && intent != Intent::None => {
            assert!(
                accepted.contains(&intent),
                "confident misroute: {phrase:?} -> {intent:?} (conf {conf:.2}), \
                 accepted: {accepted:?}"
            );
        }
        // Deferred (low confidence, reject class, or no prediction): the
        // LLM tie-breaker handles it. Always acceptable on-device behavior.
        _ => {}
    }
}

// ────── agent cue: deterministic, beats every classifier ──────

#[test]
fn cue_phrases_route_to_agent_with_task_stripped() {
    assert_eq!(
        agent_cue("peeky agent, open the lease pdf I downloaded"),
        Some("open the lease pdf I downloaded")
    );
    // A cue utterance full of integration vocab still must not reach the
    // classifiers; the cue wins before they run.
    assert_eq!(
        agent_cue("Peeky agent, check my email and play some music"),
        Some("check my email and play some music")
    );
}

#[test]
fn non_cue_phrases_do_not_match_the_cue() {
    assert_eq!(agent_cue("open the lease pdf"), None);
    assert_eq!(agent_cue("hey peeky agent, do a thing"), None);
}

// ────── keyword allowlist: bare transport commands only ──────

#[test]
fn bare_transport_commands_short_circuit_to_integration() {
    assert_eq!(keyword_classify("play"), Some(Intent::Integration));
    assert_eq!(keyword_classify("skip"), Some(Intent::Integration));
}

#[test]
fn transport_words_inside_sentences_fall_through_to_routelet() {
    assert_eq!(keyword_classify("play sicko mode"), None);
    assert_eq!(keyword_classify("can you pause for a second"), None);
}

// ────── routelet golden set: screen actions ──────

#[test]
fn screen_action_phrases_do_not_confidently_misroute() {
    // From live transcripts: all routed FindAction 0.99 on-device.
    for phrase in [
        "Can you highlight the word pets",
        "Can you highlight pets on this PDF, please?",
        "Can you scroll down and and find where it says pets?",
        "Can you find where it says pets on this PDF and then scroll down?",
        "Can you type in the words Brooks here on the notes app?",
    ] {
        assert_no_confident_misroute(phrase, &[Intent::FindAction]);
    }
    // Classic phrasings from the intent docs.
    for phrase in [
        "where is the export button",
        "click the blue button",
        "show me where the settings are",
    ] {
        assert_no_confident_misroute(phrase, &[Intent::FindAction]);
    }
}

// ────── routelet golden set: file find/open ──────
// Acceptable on two paths: Agent chains spotlight_search + finder_open, and
// since the integration path got a bounded tool loop, Integration completes
// the same chain. Both toolboxes can finish the task.

#[test]
fn file_find_open_phrases_do_not_confidently_misroute() {
    for phrase in [
        "Find that lease PDF I downloaded and open it",
        "Open the lease agreement PDF.",
        "Can you find the lease agreement PDF and open it",
        "Can you open up my lease agreement PDF and open it for me?",
    ] {
        assert_no_confident_misroute(phrase, &[Intent::Agent, Intent::Integration]);
    }
}

// ────── routelet golden set: services, chat, memory ──────

#[test]
fn service_phrases_do_not_confidently_misroute() {
    for phrase in [
        "check my email",
        "show my open pull requests",
        "play some lofi on spotify",
    ] {
        assert_no_confident_misroute(phrase, &[Intent::Integration]);
    }
}

#[test]
fn chat_phrases_do_not_confidently_misroute() {
    for phrase in [
        "Hello. What is your name?",
        "explain how rust ownership works",
        "what do you think about this plan",
    ] {
        assert_no_confident_misroute(phrase, &[Intent::Chat]);
    }
}

#[test]
fn memory_phrases_do_not_confidently_misroute() {
    for phrase in ["remember my name is Daniel", "what's my name?"] {
        assert_no_confident_misroute(phrase, &[Intent::Memory]);
    }
}

// ────── known misroutes: the next retrain's worklist ──────
// These FAIL on the shipped model and document real bugs observed live.
// Unignore each one when a retrain fixes it. Run with `cargo test -- --ignored`.

#[test]
#[ignore = "shipped model says Integration 0.99; needs type-action vocab in retrain"]
fn typing_with_deictic_here_is_a_screen_action() {
    // Live transcript 2026-06-12: routed Integration at 0.99, model
    // improvised clipboard_write because that path cannot type.
    assert_no_confident_misroute(
        "Can you type in Daniel Brooks here, please?",
        &[Intent::FindAction],
    );
}

#[test]
#[ignore = "shipped model picks Integration for a truncated fragment; should defer"]
fn truncated_fragments_should_defer_not_guess() {
    // Live transcript 2026-06-12: STT cut the utterance at "Can you
    // highlight"; routelet confidently sent it to Integration, which
    // picked safari_list_tabs. Fragments with no object should defer.
    assert_no_confident_misroute("Can you highlight", &[Intent::FindAction]);
}
