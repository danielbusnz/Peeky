//! Explicit agent cue detector. The multi-step agent is no longer one of the
//! classified intents; it is invoked only when the transcript opens with a cue
//! phrase. This module is the deterministic recognizer for that phrase: it
//! matches the cue at the start of the utterance and returns the remaining task
//! text, so the orchestrator can route to the agent without ever asking the
//! classifier.
//!
//! Start-anchored on purpose: the cue is a mode marker the user says first
//! ("peeky agent, open youtube and play the top result"), so routing can happen
//! before the rest of the utterance is processed. A cue word buried mid-sentence
//! ("ask the agent why") is not a mode switch and must not match.

// The wake phrase that spawns the agent. Provisional: "peeky agent" is the
// readable choice, but "Peeky" mis-transcribes often (Peaky, picky, peeking), so
// the final phrase should be picked from real STT logs. Whatever it becomes,
// keep it a single phrase with no aliases. Stored lowercase; matching is
// case-insensitive.
const AGENT_CUE: &str = "peeky agent";

/// If `transcript` begins with the agent cue, return the task text that follows
/// it (trimmed, with any separating punctuation removed). Returns `None` when
/// the cue is absent or only a prefix of a longer word ("peeky agentic").
///
/// A bare cue with no task ("peeky agent") returns `Some("")`; the caller decides
/// how to handle an empty task.
pub fn agent_cue(transcript: &str) -> Option<&str> {
    let trimmed = transcript.trim();

    // Case-insensitive prefix match. `get(..len)` is None if the cue is longer
    // than the input or would split a multi-byte char, so the later slice is safe.
    let head = trimmed.get(..AGENT_CUE.len())?;
    if !head.eq_ignore_ascii_case(AGENT_CUE) {
        return None;
    }
    let rest = &trimmed[AGENT_CUE.len()..];

    // Require a word boundary after the cue so "peeky agentic" does not match.
    // The boundary is end-of-string or any non-alphanumeric char (space, comma).
    match rest.chars().next() {
        None => Some(""),
        Some(c) if !c.is_alphanumeric() => Some(
            rest.trim_start_matches(|c: char| c.is_whitespace() || c.is_ascii_punctuation())
                .trim(),
        ),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cue_with_task_returns_task() {
        assert_eq!(
            agent_cue("peeky agent, open youtube and play the top lofi result"),
            Some("open youtube and play the top lofi result")
        );
        assert_eq!(agent_cue("peeky agent open youtube"), Some("open youtube"));
    }

    #[test]
    fn cue_is_case_insensitive_but_task_keeps_its_casing() {
        assert_eq!(agent_cue("Peeky Agent, open YouTube"), Some("open YouTube"));
        assert_eq!(agent_cue("PEEKY AGENT do X"), Some("do X"));
    }

    #[test]
    fn bare_cue_returns_empty_task() {
        assert_eq!(agent_cue("peeky agent"), Some(""));
        assert_eq!(agent_cue("peeky agent."), Some(""));
        assert_eq!(agent_cue("  peeky agent  "), Some(""));
    }

    #[test]
    fn leading_and_trailing_whitespace_is_ignored() {
        assert_eq!(
            agent_cue("   peeky agent   do the thing  "),
            Some("do the thing")
        );
    }

    #[test]
    fn no_cue_returns_none() {
        assert_eq!(agent_cue("open youtube and play lofi"), None);
        assert_eq!(agent_cue("what's the weather"), None);
        assert_eq!(agent_cue(""), None);
    }

    #[test]
    fn cue_must_be_at_the_start() {
        assert_eq!(agent_cue("hey peeky agent, do X"), None);
        assert_eq!(agent_cue("ask the agent why"), None);
    }

    #[test]
    fn cue_as_prefix_of_a_longer_word_does_not_match() {
        assert_eq!(agent_cue("peeky agentic workflows"), None);
    }
}
