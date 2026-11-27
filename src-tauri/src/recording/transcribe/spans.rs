//! Which audio whisper sees: pure planning over voice activity spans.
//!
//! Positions are sample indices into one buffer of 16 kHz audio. Voice
//! activity detection (Silero) finds raw speech spans without padding; this
//! module pads them by [`policy::VAD_PAD_BEFORE_MS`] and
//! [`policy::VAD_PAD_AFTER_MS`], keeps each one at most
//! [`policy::VAD_MAX_SPAN_MS`] long (whisper drops sentences from longer
//! inputs with timestamp tokens off, and the spike kept spans at 30 s), and
//! decides how much of a segment file's end to hold back for the next file
//! ([`hold_from`]).

use crate::recording::policy;

/// A range of samples, `start..end`.
pub type Span = (usize, usize);

pub const fn ms_to_samples(ms: u32) -> usize {
    ms as usize * (policy::OUTPUT_SAMPLE_RATE as usize / 1000)
}

pub const fn samples_to_ms(samples: usize) -> u64 {
    samples as u64 * 1000 / policy::OUTPUT_SAMPLE_RATE as u64
}

fn max_span() -> usize {
    ms_to_samples(policy::VAD_MAX_SPAN_MS)
}

/// Raw spans clamped to `0..len`, empty ones dropped, in order.
fn sanitize(raw: &[Span], len: usize) -> Vec<Span> {
    let mut spans: Vec<Span> = raw
        .iter()
        .map(|&(start, end)| (start.min(len), end.min(len)))
        .filter(|(start, end)| end > start)
        .collect();
    spans.sort_unstable();
    spans
}

/// The spans to hand whisper for raw speech spans in a buffer of `len`
/// samples: padded, split into pieces no longer than the maximum, and merged
/// where they meet as long as the result still fits. Where two padded spans
/// overlap but can't merge, they meet halfway through the overlap. The
/// result is in order, non-overlapping, and every span is at most
/// [`policy::VAD_MAX_SPAN_MS`] long.
pub fn plan_spans(raw: &[Span], len: usize) -> Vec<Span> {
    let before = ms_to_samples(policy::VAD_PAD_BEFORE_MS);
    let after = ms_to_samples(policy::VAD_PAD_AFTER_MS);
    let max = max_span();
    let mut padded = Vec::new();
    for (start, end) in sanitize(raw, len) {
        let (start, end) = (start.saturating_sub(before), (end + after).min(len));
        // Voice activity detection already splits long speech; this only
        // guards the limit if it didn't.
        let pieces = (end - start).div_ceil(max);
        let step = (end - start).div_ceil(pieces);
        let mut at = start;
        while at < end {
            padded.push((at, (at + step).min(end)));
            at += step;
        }
    }
    let mut planned: Vec<Span> = Vec::new();
    for (start, end) in padded {
        match planned.last_mut() {
            Some(last) if start <= last.1 => {
                if end - last.0 <= max {
                    last.1 = last.1.max(end);
                } else {
                    let middle = (start + last.1) / 2;
                    last.1 = middle;
                    if end > middle {
                        planned.push((middle, end));
                    }
                }
            }
            _ => planned.push((start, end)),
        }
    }
    planned.retain(|(start, end)| end > start);
    planned
}

/// Where to stop transcribing a buffer whose run of audio goes on into the
/// next segment file. Everything from the returned index on is held back
/// and transcribed with the next file, so speech is never cut at a file
/// boundary:
///
/// - the last [`policy::VAD_PAD_BEFORE_MS`] plus
///   [`policy::VAD_MIN_SPEECH_MS`] are always held, because a word starting
///   there is too short to be detected yet. That is longer than
///   [`policy::VAD_MIN_SILENCE_MS`], so speech that may still be going on
///   at the end (it hasn't been followed by enough silence to end) always
///   reaches into the held tail;
/// - a speech span that reaches into what is held is held whole, from its
///   padded start, and so on back while spans keep reaching the hold point.
///
/// What is held never exceeds one span, so a run of endless speech still
/// moves on.
pub fn hold_from(raw: &[Span], len: usize) -> usize {
    let spans = sanitize(raw, len);
    let before = ms_to_samples(policy::VAD_PAD_BEFORE_MS);
    let mut hold = len.saturating_sub(ms_to_samples(
        policy::VAD_PAD_BEFORE_MS + policy::VAD_MIN_SPEECH_MS,
    ));
    for &(start, end) in spans.iter().rev() {
        if end > hold {
            hold = hold.min(start.saturating_sub(before));
        }
    }
    hold.max(len.saturating_sub(max_span()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: usize = 16_000; // samples per second
    const MS: usize = 16;

    fn pad_before() -> usize {
        ms_to_samples(policy::VAD_PAD_BEFORE_MS)
    }

    fn pad_after() -> usize {
        ms_to_samples(policy::VAD_PAD_AFTER_MS)
    }

    #[test]
    fn conversions_use_16_khz() {
        assert_eq!(ms_to_samples(1000), S);
        assert_eq!(samples_to_ms(S), 1000);
        assert_eq!(samples_to_ms(15), 0);
        assert_eq!(samples_to_ms(16), 1);
    }

    #[test]
    fn spans_are_padded_and_clamped_to_the_buffer() {
        let len = 10 * S;
        assert_eq!(
            plan_spans(&[(2 * S, 3 * S)], len),
            vec![(2 * S - pad_before(), 3 * S + pad_after())]
        );
        assert_eq!(
            plan_spans(&[(100, 9 * S + 900 * MS)], len),
            vec![(0, len)],
            "padding stops at both ends"
        );
        assert!(plan_spans(&[], len).is_empty());
        assert!(plan_spans(&[(5, 5), (len + 1, len + 9)], len).is_empty());
    }

    #[test]
    fn close_spans_merge_while_they_fit() {
        let len = 60 * S;
        let planned = plan_spans(
            &[(4 * S, 5 * S), (1 * S, 2 * S), (2 * S + 200 * MS, 3 * S)],
            len,
        );
        assert_eq!(
            planned,
            vec![
                (S - pad_before(), 3 * S + pad_after()),
                (4 * S - pad_before(), 5 * S + pad_after())
            ],
            "sorted, and the two spans 200 ms apart merged"
        );
    }

    #[test]
    fn overlapping_spans_that_cannot_merge_meet_halfway() {
        let len = 60 * S;
        // 20 s of speech, 0.5 s of quiet, 20 s more: padding makes them
        // overlap, and together they are over 30 s.
        let first = (S, 21 * S);
        let second = (21 * S + 500 * MS, 41 * S + 500 * MS);
        let planned = plan_spans(&[first, second], len);
        assert_eq!(planned.len(), 2);
        let middle = (second.0 - pad_before() + first.1 + pad_after()) / 2;
        assert_eq!(planned[0], (first.0 - pad_before(), middle));
        assert_eq!(planned[1], (middle, second.1 + pad_after()));
    }

    #[test]
    fn spans_never_exceed_the_maximum_below_at_and_above() {
        let len = 200 * S;
        let max = max_span();
        for speech in [
            max - pad_before() - pad_after() - 1,
            max - pad_before() - pad_after(),
            max,
            95 * S,
        ] {
            let planned = plan_spans(&[(10 * S, 10 * S + speech)], len);
            assert!(
                planned.iter().all(|(a, b)| b - a <= max),
                "{speech}: {planned:?}"
            );
            let covered: usize = planned.iter().map(|(a, b)| b - a).sum();
            assert_eq!(covered, speech + pad_before() + pad_after(), "nothing lost");
            let expected_pieces = if speech + pad_before() + pad_after() <= max {
                1
            } else {
                (speech + pad_before() + pad_after()).div_ceil(max)
            };
            assert_eq!(planned.len(), expected_pieces, "{speech}");
        }
    }

    #[test]
    fn a_quiet_end_holds_only_the_short_tail() {
        let len = 60 * S;
        let tail = ms_to_samples(policy::VAD_PAD_BEFORE_MS + policy::VAD_MIN_SPEECH_MS);
        assert_eq!(hold_from(&[], len), len - tail);
        assert_eq!(hold_from(&[(10 * S, 20 * S)], len), len - tail);
    }

    #[test]
    fn speech_reaching_the_end_is_held_from_its_padded_start() {
        let len = 60 * S;
        let tail = ms_to_samples(policy::VAD_PAD_BEFORE_MS + policy::VAD_MIN_SPEECH_MS);
        assert!(tail >= ms_to_samples(policy::VAD_MIN_SILENCE_MS));
        // Still going at the end, or not yet followed by enough silence to
        // have ended.
        for end in [
            len,
            len - ms_to_samples(policy::VAD_MIN_SILENCE_MS),
            len - tail + 1,
        ] {
            assert_eq!(
                hold_from(&[(5 * S, 10 * S), (55 * S, end)], len),
                55 * S - pad_before(),
                "{end}"
            );
        }
        // Ended before the held tail: only the tail is held.
        assert_eq!(hold_from(&[(55 * S, len - tail)], len), len - tail);
    }

    #[test]
    fn the_hold_point_never_splits_a_span() {
        let len = 60 * S;
        // A span straddling the always-held tail moves the hold to its start.
        let straddle = (len - 2 * S, len - 300 * MS);
        assert_eq!(hold_from(&[straddle], len), straddle.0 - pad_before());
        // Two spans close together at the end: both are held.
        assert_eq!(
            hold_from(&[(50 * S, 52 * S), (52 * S + 100 * MS, len)], len),
            50 * S - pad_before()
        );
    }

    #[test]
    fn what_is_held_never_exceeds_one_span() {
        let len = 60 * S;
        assert_eq!(hold_from(&[(0, len)], len), len - max_span());
        let short = 10 * S;
        assert_eq!(
            hold_from(&[(0, short)], short),
            0,
            "a short buffer may be held whole"
        );
    }
}
