//! Fast-path keyword intent classifier. Returns Some(Intent) when the
//! transcript matches a strong, unambiguous pattern. Returns None for
//! anything ambiguous. The caller falls through to the LLM classifier.
//!
//! Design choice: prefer false negatives over false positives. A miss
//! (None) costs ~700ms (LLM call) but the routing is still correct. A
//! false positive routes to the wrong path. So patterns here are
//! deliberately narrow.

use crate::providers::claude::Intent;

/// Hybrid classifier entry point. Sub-millisecond keyword check.
/// Returns the Intent if the transcript matches a high-confidence
/// pattern, None otherwise. The orchestrator falls back to the LLM
/// classifier when this returns None.
pub fn keyword_classify(transcript: &str) -> Option<Intent> {
    let lower = transcript.trim().to_lowercase();
    let padded = format!(" {} ", lower);

    // Order matters. Memory has the most distinctive phrasing
    // ("remember my X" / "what's my Y"), no overlap risk. FindAction
    // runs BEFORE Integration because location/click verbs are
    // stronger signals than a bare service name appearing somewhere
    // in the utterance: "where's my YouTube button" must route to
    // FindAction (point at the button), NOT Integration (play a video).
    // Integration matches plain service commands ("play X on Spotify",
    // "check my email") that don't have a locator verb up front.
    if matches_memory(&padded, &lower) {
        return Some(Intent::Memory);
    }
    if matches_find_action(&padded, &lower) {
        return Some(Intent::FindAction);
    }
    if matches_integration(&padded, &lower) {
        return Some(Intent::Integration);
    }
    if matches_chat(&padded, &lower) {
        return Some(Intent::Chat);
    }
    None
}

/// Memory: "remember my X is Y" / "what's my Z" / "do you remember".
/// Note "what's my..." vs "what's your...": the former is Memory,
/// the latter is Chat. Order in keyword_classify keeps them disjoint.
fn matches_memory(padded: &str, lower: &str) -> bool {
    if lower.starts_with("remember ")
        || lower.starts_with("remember,")
        || lower.starts_with("please remember ")
    {
        return true;
    }
    // Recall phrasings. Use padded to avoid false positives like "what's
    // my friend's number". Keep these narrow.
    if lower.starts_with("what's my ")
        || lower.starts_with("what is my ")
        || lower.starts_with("do you remember ")
        || lower.starts_with("did i tell you ")
    {
        return true;
    }
    if padded.contains(" what did i tell you ") || padded.contains(" remember that ") {
        return true;
    }
    false
}

/// Integration: connected services (Spotify, Gmail, GitHub, YouTube)
/// and unambiguous media commands.
fn matches_integration(padded: &str, lower: &str) -> bool {
    // Media playback commands at the start of the utterance.
    let starts = [
        "play ",
        "pause",
        "resume",
        "skip ",
        "skip,",
        "next song",
        "next track",
        "previous song",
        "previous track",
        "what's playing",
        "what is playing",
    ];
    if starts.iter().any(|p| lower.starts_with(p)) {
        return true;
    }
    // Service names. Padded so "spotifying" doesn't false-positive.
    let services = [" spotify", " gmail", " github", " youtube"];
    if services.iter().any(|s| padded.contains(s)) {
        return true;
    }
    // Common integration verbs.
    let phrases = [
        " my inbox",
        " my email",
        " unread email",
        " unread mail",
        " my pull requests",
        " my prs",
        " my pr ",
        " my issues",
        " my repos",
        " send an email",
        " send email",
        " send a message",
    ];
    if phrases.iter().any(|p| padded.contains(p)) {
        return true;
    }
    false
}

/// FindAction: cursor moves or fires input on something visible.
/// Covers pointing, clicking, typing, scrolling.
fn matches_find_action(padded: &str, lower: &str) -> bool {
    // Pointing / locating phrasings.
    let starts = [
        "where is",
        "where's",
        "where are",
        "where does it",
        "where on",
        "where can i",
        "show me where",
        "show me the",
        "find the",
        "find me the",
        "find me ",
        "point at",
        "point to",
        "click ",
        "click on",
        "tap ",
        "press ",
        "double click",
        "select the",
        "type ",
        "scroll ",
    ];
    if starts.iter().any(|p| lower.starts_with(p)) {
        return true;
    }
    // UI-element references. Very strong visual signal.
    let ui_refs = [
        " the button",
        " the icon",
        " the menu",
        " the tab",
        " the link",
        " the search bar",
        " the close button",
    ];
    if ui_refs.iter().any(|r| padded.contains(r)) {
        return true;
    }
    false
}

/// Chat: common conversational openers about Claude itself or general
/// information. Keep narrow; Chat is the LLM's default for ambiguous
/// queries, so we only short-circuit the very common patterns here.
fn matches_chat(_padded: &str, lower: &str) -> bool {
    let starts = [
        "what's your name",
        "what is your name",
        "who are you",
        "how are you",
        "how's it going",
        "what's up",
        "good morning",
        "good afternoon",
        "good evening",
        "hello",
        "hey there",
        "hi there",
        "tell me a joke",
        "tell me about yourself",
        "what can you do",
        "what do you do",
    ];
    starts.iter().any(|p| lower.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_remember_starts() {
        assert_eq!(
            keyword_classify("remember my name is Daniel"),
            Some(Intent::Memory)
        );
        assert_eq!(
            keyword_classify("remember that I'm allergic to peanuts"),
            Some(Intent::Memory)
        );
    }

    #[test]
    fn memory_what_is_my() {
        assert_eq!(
            keyword_classify("what's my favorite color"),
            Some(Intent::Memory)
        );
        assert_eq!(
            keyword_classify("what is my home city"),
            Some(Intent::Memory)
        );
    }

    #[test]
    fn integration_media_commands() {
        assert_eq!(
            keyword_classify("play despacito"),
            Some(Intent::Integration)
        );
        assert_eq!(keyword_classify("pause"), Some(Intent::Integration));
        assert_eq!(
            keyword_classify("skip this song"),
            Some(Intent::Integration)
        );
        assert_eq!(
            keyword_classify("what's playing right now"),
            Some(Intent::Integration)
        );
    }

    #[test]
    fn integration_service_names() {
        assert_eq!(
            keyword_classify("check my gmail"),
            Some(Intent::Integration)
        );
        assert_eq!(
            keyword_classify("show my github prs"),
            Some(Intent::Integration)
        );
    }

    #[test]
    fn find_action_pointing() {
        assert_eq!(
            keyword_classify("where is the search bar"),
            Some(Intent::FindAction)
        );
        assert_eq!(
            keyword_classify("where does it say the price"),
            Some(Intent::FindAction)
        );
        assert_eq!(
            keyword_classify("click the play button"),
            Some(Intent::FindAction)
        );
        assert_eq!(keyword_classify("scroll down"), Some(Intent::FindAction));
    }

    #[test]
    fn chat_identity_questions() {
        assert_eq!(keyword_classify("what's your name"), Some(Intent::Chat));
        assert_eq!(keyword_classify("who are you"), Some(Intent::Chat));
        assert_eq!(keyword_classify("how are you"), Some(Intent::Chat));
    }

    #[test]
    fn ambiguous_returns_none() {
        // No clear pattern → fall through to LLM.
        assert_eq!(keyword_classify("can you help me with something"), None);
        assert_eq!(keyword_classify("I need to do a thing"), None);
        assert_eq!(keyword_classify(""), None);
    }

    #[test]
    fn find_action_beats_integration_on_locator_verbs() {
        // Real-world case: user asks "where's my YouTube button?". They
        // want the cursor to point at the button, NOT for YouTube to
        // start playing a video. "Where's" is a stronger signal than
        // "YouTube" being mentioned anywhere in the utterance.
        assert_eq!(
            keyword_classify("Where's my YouTube button?"),
            Some(Intent::FindAction)
        );
        assert_eq!(
            keyword_classify("where is the spotify icon"),
            Some(Intent::FindAction)
        );
        assert_eq!(
            keyword_classify("click the github tab"),
            Some(Intent::FindAction)
        );
        // Bare service commands stay Integration as expected.
        assert_eq!(
            keyword_classify("play despacito on spotify"),
            Some(Intent::Integration)
        );
    }

    #[test]
    fn memory_vs_chat_disambiguation() {
        // "what's MY name" → Memory (asking about themselves).
        // "what's YOUR name" → Chat (asking about Claude).
        // Subtle but critical distinction.
        assert_eq!(keyword_classify("what's my name"), Some(Intent::Memory));
        assert_eq!(keyword_classify("what's your name"), Some(Intent::Chat));
    }

    #[test]
    fn memory_please_remember_prefix() {
        assert_eq!(
            keyword_classify("please remember my birthday is March 5"),
            Some(Intent::Memory)
        );
    }

    #[test]
    fn memory_did_i_tell_you() {
        assert_eq!(
            keyword_classify("did I tell you my favorite food"),
            Some(Intent::Memory)
        );
    }

    #[test]
    fn memory_do_you_remember() {
        assert_eq!(
            keyword_classify("do you remember my home address"),
            Some(Intent::Memory)
        );
    }

    #[test]
    fn integration_next_and_previous_track() {
        assert_eq!(keyword_classify("next song"), Some(Intent::Integration));
        assert_eq!(keyword_classify("next track"), Some(Intent::Integration));
        assert_eq!(keyword_classify("previous song"), Some(Intent::Integration));
        assert_eq!(keyword_classify("previous track"), Some(Intent::Integration));
    }

    #[test]
    fn integration_resume() {
        assert_eq!(keyword_classify("resume"), Some(Intent::Integration));
    }

    #[test]
    fn integration_email_and_pr_phrases() {
        assert_eq!(
            keyword_classify("check my inbox"),
            Some(Intent::Integration)
        );
        assert_eq!(
            keyword_classify("show my unread email"),
            Some(Intent::Integration)
        );
        assert_eq!(
            keyword_classify("what are my pull requests"),
            Some(Intent::Integration)
        );
        assert_eq!(
            keyword_classify("list my repos"),
            Some(Intent::Integration)
        );
    }

    #[test]
    fn integration_youtube_service_name() {
        assert_eq!(
            keyword_classify("play something on youtube"),
            Some(Intent::Integration)
        );
    }

    #[test]
    fn find_action_ui_element_references() {
        assert_eq!(
            keyword_classify("click on the button"),
            Some(Intent::FindAction)
        );
        assert_eq!(
            keyword_classify("open the menu"),
            Some(Intent::FindAction)
        );
        assert_eq!(
            keyword_classify("focus the search bar"),
            // "the search bar" is a UI ref → FindAction
            Some(Intent::FindAction)
        );
    }

    #[test]
    fn find_action_type_and_scroll_prefixes() {
        assert_eq!(
            keyword_classify("type hello into the box"),
            Some(Intent::FindAction)
        );
        assert_eq!(
            keyword_classify("scroll up a bit"),
            Some(Intent::FindAction)
        );
    }

    #[test]
    fn find_action_select_prefix() {
        assert_eq!(
            keyword_classify("select the text"),
            Some(Intent::FindAction)
        );
    }

    #[test]
    fn find_action_show_me_where() {
        assert_eq!(
            keyword_classify("show me where the settings are"),
            Some(Intent::FindAction)
        );
    }

    #[test]
    fn chat_greeting_variants() {
        assert_eq!(keyword_classify("hello"), Some(Intent::Chat));
        assert_eq!(keyword_classify("good morning"), Some(Intent::Chat));
        assert_eq!(keyword_classify("good afternoon"), Some(Intent::Chat));
        assert_eq!(keyword_classify("good evening"), Some(Intent::Chat));
        assert_eq!(keyword_classify("hey there"), Some(Intent::Chat));
        assert_eq!(keyword_classify("hi there"), Some(Intent::Chat));
    }

    #[test]
    fn chat_identity_variants() {
        assert_eq!(keyword_classify("tell me a joke"), Some(Intent::Chat));
        assert_eq!(keyword_classify("what can you do"), Some(Intent::Chat));
        assert_eq!(keyword_classify("what do you do"), Some(Intent::Chat));
        assert_eq!(
            keyword_classify("tell me about yourself"),
            Some(Intent::Chat)
        );
    }

    #[test]
    fn keyword_classify_is_case_insensitive() {
        // The classifier lowercases input before matching.
        assert_eq!(
            keyword_classify("REMEMBER MY NAME IS ALICE"),
            Some(Intent::Memory)
        );
        assert_eq!(
            keyword_classify("PLAY Some Music"),
            Some(Intent::Integration)
        );
        assert_eq!(
            keyword_classify("CLICK THE BUTTON"),
            Some(Intent::FindAction)
        );
    }

    #[test]
    fn keyword_classify_trims_leading_trailing_whitespace() {
        assert_eq!(
            keyword_classify("  remember my color is red  "),
            Some(Intent::Memory)
        );
    }
}
