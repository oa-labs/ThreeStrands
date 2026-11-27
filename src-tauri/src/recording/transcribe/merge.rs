//! From both tracks' raw whisper lines to one transcript: a post-filter per
//! track, then a merge in time order that marks microphone lines which only
//! repeat the computer's audio (speaker echo).
//!
//! Pure code, so every rule is tested directly.
//!
//! **Echo.** Without headphones the microphone hears the other people
//! through the speakers, and whisper transcribes that echo almost word for
//! word. A microphone line is dropped as echo when at least
//! [`policy::ECHO_DROP_THRESHOLD`] of its words also appear in system lines
//! that overlap it in time, ± [`policy::MERGE_TIME_SLACK_MS`]. The spike
//! measured echo at 0.70–1.00 and the user's own line spoken over the remote
//! speaker at 0.25. The plan exempts recordings where every output is
//! headphones (nothing is echo then); that needs output-route detection and
//! is deferred, so the rule always applies when there is system audio.

use super::protocol::TranscriptLine;
use crate::recording::capture::Track;
use crate::recording::policy;
use std::collections::HashSet;

/// A line of the merged transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedLine {
    pub line: TranscriptLine,
    pub dropped_as_echo: bool,
}

/// Lowercase words with punctuation stripped.
pub fn normalized_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

/// Text wholly inside one pair of brackets or parentheses, such as
/// "[BLANK_AUDIO]", "(music)", or "[ Laughter ]": whisper's labels for
/// non-speech.
fn is_bracketed(text: &str) -> bool {
    let wrapped = |open: char, close: char| {
        text.starts_with(open)
            && text.ends_with(close)
            && text.len() >= 2
            && !text[1..text.len() - 1].contains([open, close])
    };
    wrapped('[', ']') || wrapped('(', ')') || wrapped('*', '*')
}

fn is_hallucination(text: &str, words: &[String]) -> bool {
    is_bracketed(text)
        || policy::HALLUCINATIONS
            .iter()
            .any(|known| normalized_words(known) == words)
}

/// One track's lines with whitespace trimmed, and empty lines, known
/// hallucinations, non-speech labels, and consecutive repeats dropped.
/// Order is kept.
pub fn post_filter(lines: Vec<TranscriptLine>) -> Vec<TranscriptLine> {
    let mut kept: Vec<TranscriptLine> = Vec::new();
    let mut previous: Option<Vec<String>> = None;
    for mut line in lines {
        line.text = line.text.trim().to_string();
        let words = normalized_words(&line.text);
        if words.is_empty() || is_hallucination(&line.text, &words) {
            continue;
        }
        if previous.as_ref() == Some(&words) {
            continue;
        }
        previous = Some(words);
        kept.push(line);
    }
    kept
}

/// The share (0 to 1) of `mic`'s words found in the system lines that
/// overlap it in time, with slack.
pub fn echo_score(mic: &TranscriptLine, system: &[TranscriptLine]) -> f32 {
    let slack = policy::MERGE_TIME_SLACK_MS.max(0) as u64;
    let heard: HashSet<String> = system
        .iter()
        .filter(|line| {
            line.start_ms <= mic.end_ms.saturating_add(slack)
                && line.end_ms.saturating_add(slack) >= mic.start_ms
        })
        .flat_map(|line| normalized_words(&line.text))
        .collect();
    let words = normalized_words(&mic.text);
    if words.is_empty() {
        return 0.0;
    }
    let found = words.iter().filter(|word| heard.contains(*word)).count();
    found as f32 / words.len() as f32
}

/// Filters each track, orders all lines by start time (the microphone first
/// on a tie, then by end), and marks microphone echo. Echo is only judged
/// when the system track has lines.
pub fn merge(lines: Vec<TranscriptLine>) -> Vec<MergedLine> {
    let (mic, system): (Vec<_>, Vec<_>) = lines.into_iter().partition(|l| l.track == Track::Mic);
    let mut mic = post_filter(mic);
    let mut system = post_filter(system);
    let by_time = |a: &TranscriptLine, b: &TranscriptLine| {
        (a.start_ms, a.end_ms).cmp(&(b.start_ms, b.end_ms))
    };
    mic.sort_by(by_time);
    system.sort_by(by_time);
    let mut merged: Vec<MergedLine> = mic
        .into_iter()
        .map(|line| MergedLine {
            dropped_as_echo: !system.is_empty()
                && echo_score(&line, &system) >= policy::ECHO_DROP_THRESHOLD,
            line,
        })
        .collect();
    merged.extend(system.into_iter().map(|line| MergedLine {
        line,
        dropped_as_echo: false,
    }));
    merged.sort_by(|a, b| {
        (a.line.start_ms, a.line.track, a.line.end_ms).cmp(&(
            b.line.start_ms,
            b.line.track,
            b.line.end_ms,
        ))
    });
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(track: Track, start_ms: u64, end_ms: u64, text: &str) -> TranscriptLine {
        TranscriptLine {
            track,
            start_ms,
            end_ms,
            text: text.into(),
        }
    }

    fn mic(start: u64, end: u64, text: &str) -> TranscriptLine {
        line(Track::Mic, start, end, text)
    }

    fn system(start: u64, end: u64, text: &str) -> TranscriptLine {
        line(Track::System, start, end, text)
    }

    fn texts(lines: &[TranscriptLine]) -> Vec<&str> {
        lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn words_are_lowercased_without_punctuation() {
        assert_eq!(
            normalized_words("  Let's ship it, OK?  Q3 — done. "),
            ["lets", "ship", "it", "ok", "q3", "done"]
        );
        assert!(normalized_words("… — !").is_empty());
    }

    #[test]
    fn the_post_filter_trims_and_drops_empty_lines_and_hallucinations() {
        let kept = post_filter(vec![
            mic(0, 1, "  Hello there.  "),
            mic(1, 2, "   "),
            mic(2, 3, "Thank you."),
            mic(3, 4, "thank you"),
            mic(4, 5, "Thanks for watching!"),
            mic(5, 6, "[BLANK_AUDIO]"),
            mic(6, 7, "(music)"),
            mic(7, 8, "[ Laughter ]"),
            mic(8, 9, "*sighs*"),
            mic(9, 10, "♪ ♪"),
            mic(10, 11, "you"),
            mic(11, 12, "Thank you for the update on the budget."),
            mic(12, 13, "(I think) we agree."),
        ]);
        assert_eq!(
            texts(&kept),
            [
                "Hello there.",
                "Thank you for the update on the budget.",
                "(I think) we agree."
            ]
        );
    }

    #[test]
    fn consecutive_repeats_within_a_track_are_dropped() {
        let kept = post_filter(vec![
            mic(0, 1, "We should ship."),
            mic(1, 2, "we should ship"),
            mic(2, 3, "We should ship!"),
            mic(3, 4, "Agreed."),
            mic(4, 5, "We should ship."),
        ]);
        assert_eq!(
            texts(&kept),
            ["We should ship.", "Agreed.", "We should ship."],
            "only consecutive repeats"
        );
        // Repeats across tracks are not repeats.
        let merged = merge(vec![mic(0, 1000, "Yes."), system(5000, 6000, "Yes.")]);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn lines_are_ordered_by_start_with_the_microphone_first_on_a_tie() {
        let merged = merge(vec![
            system(5_000, 6_000, "Third."),
            mic(9_000, 9_500, "Fourth."),
            system(0, 1_000, "Second."),
            mic(0, 2_000, "First."),
        ]);
        let order: Vec<&str> = merged.iter().map(|m| m.line.text.as_str()).collect();
        assert_eq!(order, ["First.", "Second.", "Third.", "Fourth."]);
        assert!(merged.iter().all(|m| !m.dropped_as_echo), "no shared words");
    }

    #[test]
    fn echo_is_scored_on_words_heard_in_overlapping_system_lines() {
        let remote = [system(
            10_000,
            14_000,
            "The launch moves to March fifteenth.",
        )];
        let echo = mic(10_060, 14_060, "The launch moves to March fifteenth");
        assert_eq!(echo_score(&echo, &remote), 1.0);
        let own = mic(12_000, 13_000, "I'll tell the launch team");
        assert_eq!(echo_score(&own, &remote), 0.4, "the, launch");
        // The same words outside the time window don't count.
        let later = mic(15_001, 16_000, "The launch moves to March fifteenth");
        assert_eq!(echo_score(&later, &remote), 0.0);
        let within_slack = mic(15_000, 16_000, "The launch moves to March fifteenth");
        assert_eq!(echo_score(&within_slack, &remote), 1.0);
        let before = mic(8_000, 9_000, "The launch moves");
        assert_eq!(
            echo_score(&before, &remote),
            1.0,
            "ends 1 s before: inside the slack"
        );
        let too_early = mic(8_000, 8_999, "The launch moves");
        assert_eq!(echo_score(&too_early, &remote), 0.0);
    }

    /// A microphone line with `found` of its 10 words in the system line.
    fn scored(found: usize) -> Vec<MergedLine> {
        let remote: Vec<String> = (0..10).map(|i| format!("remote{i}")).collect();
        let words: Vec<String> = (0..10)
            .map(|i| {
                if i < found {
                    format!("remote{i}")
                } else {
                    format!("own{i}")
                }
            })
            .collect();
        merge(vec![
            system(0, 5_000, &remote.join(" ")),
            mic(100, 5_100, &words.join(" ")),
        ])
    }

    #[test]
    fn echo_is_dropped_below_at_and_above_the_threshold() {
        assert_eq!(policy::ECHO_DROP_THRESHOLD, 0.5);
        let mic_line = |merged: &[MergedLine]| {
            merged
                .iter()
                .find(|m| m.line.track == Track::Mic)
                .unwrap()
                .dropped_as_echo
        };
        assert!(!mic_line(&scored(4)), "0.4 is kept");
        assert!(mic_line(&scored(5)), "0.5 is echo");
        assert!(mic_line(&scored(6)), "0.6 is echo");
        assert!(
            scored(10)
                .iter()
                .all(|m| m.line.track == Track::Mic || !m.dropped_as_echo),
            "system lines are never dropped"
        );
    }

    #[test]
    fn without_system_lines_nothing_is_echo() {
        let merged = merge(vec![
            mic(0, 1_000, "Hello everyone."),
            mic(1_000, 2_000, "Let's start."),
        ]);
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().all(|m| !m.dropped_as_echo));
        // A system track of only hallucinations counts as no system lines.
        let merged = merge(vec![
            mic(0, 1_000, "Thanks everyone."),
            system(0, 1_000, "[BLANK_AUDIO]"),
        ]);
        assert_eq!(merged.len(), 1);
        assert!(!merged[0].dropped_as_echo);
    }

    #[test]
    fn echo_is_judged_after_the_post_filter() {
        // The system line is a hallucination, so it is gone before the
        // microphone line is scored against it.
        let merged = merge(vec![
            system(0, 1_000, "Thank you."),
            system(30_000, 31_000, "Something else."),
            mic(0, 1_000, "Thank you, all of you."),
        ]);
        let mic_line = merged.iter().find(|m| m.line.track == Track::Mic).unwrap();
        assert!(!mic_line.dropped_as_echo);
    }
}
