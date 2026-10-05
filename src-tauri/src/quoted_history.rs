//! Repeated quoted history in plain-text message bodies.
//!
//! Follows the plain-text path of the reader's quote folding
//! (`src/quotedHistory.ts`) and shares its limits (`EMAIL_QUOTE_FOLDING_LIMITS`
//! in `src/emailRenderingPolicy.ts`; a test keeps the two in step), with one
//! stricter rule: text is removable only when it repeats text the caller has
//! already recorded with [`ThreadHistory::remember`]. Structure alone — an
//! "On … wrote:" line with no recorded copy of what follows — never removes
//! anything, because that quote may be the only copy of a message that is not
//! stored locally or not being sent.

use std::collections::HashSet;

pub const SHINGLE_WORDS: usize = 4;
pub const MIN_SEEN_LINE_COVERAGE: f64 = 0.8;
pub const MIN_REPEATED_REGION_SHINGLES: usize = 8;
pub const MIN_CORROBORATING_SHINGLES: usize = 3;
pub const MIN_QUOTE_RUN_LINES: usize = 5;
pub const WRAPPED_ATTRIBUTION_LOOKAHEAD_LINES: usize = 3;
pub const MAX_ATTRIBUTION_LENGTH: usize = 500;
pub const MAX_HEADER_CLUSTER_LINES: usize = 12;

/// Shingles of the text already recorded for a thread, oldest message first.
#[derive(Default)]
pub struct ThreadHistory {
    shingles: HashSet<String>,
}

impl ThreadHistory {
    /// Records text the reader of this thread (or the model) already has.
    pub fn remember(&mut self, text: &str) {
        let words: Vec<String> = text.lines().flat_map(line_words).collect();
        for window in words.windows(SHINGLE_WORDS) {
            self.shingles.insert(window.join(" "));
        }
    }

    /// Byte length of `body`'s new text: everything before a trailing region
    /// that repeats recorded text, or `body.len()` when nothing qualifies.
    ///
    /// - When an attribution, `>` run or header block starts a trailing region
    ///   whose text is all recorded (at least `MIN_CORROBORATING_SHINGLES`
    ///   matches), the region goes from that structure, plus a recorded
    ///   signature directly above it.
    /// - Without agreeing structure, a trailing recorded run needs
    ///   `MIN_REPEATED_REGION_SHINGLES` matches.
    /// - Some new text always remains; a fully repeated body is kept whole.
    pub fn new_text_len(&self, body: &str) -> usize {
        let lines = split_lines(body);
        let texts: Vec<&str> = lines.iter().map(|(_, text)| *text).collect();
        let kinds = self.classify(&texts);
        let (top, matched) = repeated_run_above(&kinds, texts.len());
        if top == texts.len() {
            return body.len();
        }
        let has_words_before = |line: usize| texts[..line].iter().any(|text| !line_words(text).is_empty());

        let mut candidates = Vec::new();
        if let Some((cut, after)) = structural_cut(&texts) {
            // Structure agrees when only the attribution block itself (and
            // blank lines) separate it from the recorded run.
            let agrees = top < cut || kinds[after.min(top)..top].iter().all(|line| line.kind == LineKind::Neutral);
            if agrees && matched >= MIN_CORROBORATING_SHINGLES {
                if top < cut {
                    candidates.push(top);
                } else {
                    let (above, above_matched) = repeated_run_above(&kinds, cut);
                    if above < cut && above_matched >= MIN_CORROBORATING_SHINGLES {
                        candidates.push(above);
                    }
                    candidates.push(cut);
                }
            }
        }
        if matched >= MIN_REPEATED_REGION_SHINGLES {
            candidates.push(top);
        }
        candidates
            .into_iter()
            .find(|&line| has_words_before(line))
            .map_or(body.len(), |line| lines[line].0)
    }

    fn classify(&self, lines: &[&str]) -> Vec<Repeated> {
        let mut words: Vec<(String, usize)> = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            words.extend(line_words(line).into_iter().map(|word| (word, index)));
        }
        let mut covered = vec![false; words.len()];
        let mut matched = vec![0usize; lines.len()];
        for start in 0..words.len().saturating_sub(SHINGLE_WORDS - 1) {
            let shingle = words[start..start + SHINGLE_WORDS]
                .iter()
                .map(|(word, _)| word.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            if self.shingles.contains(&shingle) {
                matched[words[start].1] += 1;
                covered[start..start + SHINGLE_WORDS].fill(true);
            }
        }
        let mut totals = vec![0usize; lines.len()];
        let mut covered_counts = vec![0usize; lines.len()];
        for (index, (_, line)) in words.iter().enumerate() {
            totals[*line] += 1;
            if covered[index] {
                covered_counts[*line] += 1;
            }
        }
        (0..lines.len())
            .map(|line| Repeated {
                kind: if totals[line] == 0 {
                    LineKind::Neutral
                } else if covered_counts[line] as f64 / totals[line] as f64 >= MIN_SEEN_LINE_COVERAGE {
                    LineKind::Seen
                } else {
                    LineKind::New
                },
                matched: matched[line],
            })
            .collect()
    }
}

/// A thread's message bodies, oldest first, as its search row indexes them:
/// each body minus a trailing region that repeats an earlier body in the same
/// thread. The earliest copy of any text is never removed — nothing precedes
/// it — so the row loses no phrase it held before. Words in a removed region
/// that the thread has not used yet (a line counts as repeated at
/// `MIN_SEEN_LINE_COVERAGE`, not 100%) are kept, so every word stays searchable.
pub fn searchable_thread_text<'a>(bodies: impl IntoIterator<Item = &'a str>) -> String {
    let mut history = ThreadHistory::default();
    let mut used: HashSet<String> = HashSet::new();
    let mut out = String::new();
    for body in bodies {
        let new_len = history.new_text_len(body);
        let (new_text, removed) = body.split_at(new_len);
        out.push_str(new_text);
        let mut present: HashSet<String> = new_text.lines().flat_map(line_words).collect();
        for word in removed.lines().flat_map(line_words) {
            if !used.contains(&word) && present.insert(word.clone()) {
                out.push(' ');
                out.push_str(&word);
            }
        }
        out.push(' ');
        history.remember(body);
        used.extend(body.lines().flat_map(line_words));
    }
    out
}

#[derive(Clone, Copy, PartialEq)]
enum LineKind {
    Neutral,
    Seen,
    New,
}

struct Repeated {
    kind: LineKind,
    matched: usize,
}

/// The contiguous run of seen or wordless lines ending just above `end`,
/// starting at its first seen line so blank lines above it stay with the
/// new text.
fn repeated_run_above(lines: &[Repeated], end: usize) -> (usize, usize) {
    let mut top = end;
    let mut matched = 0;
    while top > 0 && lines[top - 1].kind != LineKind::New {
        top -= 1;
        matched += lines[top].matched;
    }
    while top < end && lines[top].kind == LineKind::Neutral {
        top += 1;
    }
    (top, matched)
}

/// Each line with its byte offset; a trailing `\r` is not part of the text.
fn split_lines(body: &str) -> Vec<(usize, &str)> {
    let mut offset = 0;
    body.split('\n')
        .map(|line| {
            let start = offset;
            offset += line.len() + 1;
            (start, line.strip_suffix('\r').unwrap_or(line))
        })
        .collect()
}

/// Normalized words of one line: quote markers, case and punctuation are ignored.
fn line_words(line: &str) -> Vec<String> {
    let mut rest = line;
    while let Some(after) = rest.trim_start().strip_prefix('>') {
        rest = after;
    }
    rest.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn is_separator_marker(line: &str) -> bool {
    let lower = line.trim().to_lowercase();
    let core = lower
        .trim_matches(|c: char| matches!(c, '-' | '—' | '_') || c.is_whitespace())
        .trim_end_matches(':')
        .trim_end();
    matches!(core, "original message" | "forwarded message" | "begin forwarded message")
}

fn opens_attribution(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.len() > 3 && trimmed[..3].eq_ignore_ascii_case("on ") && trimmed[3..].chars().next().is_some_and(|c| !c.is_whitespace())
}

/// When an attribution or separator starts at `index`, the line after it.
fn attribution_end(lines: &[&str], index: usize) -> Option<usize> {
    let line = lines[index].trim();
    if is_separator_marker(line) {
        return Some(index + 1);
    }
    if !opens_attribution(line) {
        return None;
    }
    let lower = line.to_lowercase();
    if lower.ends_with("wrote:") {
        return (line.len() - "wrote:".len() <= MAX_ATTRIBUTION_LENGTH + 3).then_some(index + 1);
    }
    if lower.contains("wrote:") {
        return None;
    }
    // "wrote:" (optionally after the address) hard-wrapped onto a following line.
    let mut joined = line.len();
    for (offset, next) in lines[index + 1..].iter().take(WRAPPED_ATTRIBUTION_LOOKAHEAD_LINES).enumerate() {
        let next = next.trim();
        if next.is_empty() {
            return None;
        }
        joined += 1 + next.len();
        let continuation = next.to_lowercase();
        let rest = continuation.strip_suffix("wrote:").map(str::trim_end);
        let is_continuation = rest.is_some_and(|rest| {
            rest.is_empty() || (rest.starts_with('<') && rest.ends_with('>') && rest.contains('@') && !rest.contains(char::is_whitespace))
        });
        if is_continuation {
            return (joined <= MAX_ATTRIBUTION_LENGTH).then_some(index + offset + 2);
        }
    }
    None
}

fn header_field(line: &str) -> Option<&'static str> {
    let lower = line.trim().to_lowercase();
    ["from", "sent", "date", "to", "cc", "bcc", "subject"]
        .into_iter()
        .find(|field| lower.strip_prefix(field).is_some_and(|rest| rest.trim_start().starts_with(':')))
}

fn has_address_or_time(text: &str) -> bool {
    let has_address = text.split_whitespace().any(|token| {
        token.split_once('@').is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'))
    });
    let bytes = text.as_bytes();
    let has_time = bytes.windows(4).any(|w| w[0].is_ascii_digit() && w[1] == b':' && w[2].is_ascii_digit() && w[3].is_ascii_digit());
    has_address || has_time
}

/// First line of quoted history by structure alone, with the line after its
/// attribution or header block (the cut itself for a `>` run), or None.
fn structural_cut(lines: &[&str]) -> Option<(usize, usize)> {
    let marker = (0..lines.len()).find_map(|index| attribution_end(lines, index).map(|end| (index, end)));
    let mut run = 0;
    let mut quote_run = None;
    for (index, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with('>') {
            run += 1;
            if run >= MIN_QUOTE_RUN_LINES {
                quote_run = Some((index + 1 - run, index + 1 - run));
                break;
            }
        } else {
            run = 0;
        }
    }
    match (marker, quote_run) {
        (Some(a), Some(b)) => return Some(if a.0 <= b.0 { a } else { b }),
        (Some(a), None) | (None, Some(a)) => return Some(a),
        (None, None) => {}
    }
    // A From/Sent/To/Subject block, starting at a header line, after a
    // separator line; the cut starts at the separator.
    (1..lines.len()).find_map(|index| {
        header_field(lines[index])?;
        let block: Vec<&str> = lines[index..]
            .iter()
            .take_while(|line| !line.trim().is_empty())
            .take(MAX_HEADER_CLUSTER_LINES)
            .copied()
            .collect();
        let fields: HashSet<&str> = block.iter().filter_map(|line| header_field(line)).collect();
        let separator = (0..index).rev().find(|&line| !lines[line].trim().is_empty()).filter(|&line| {
            let line = lines[line].trim();
            line.chars().count() >= 2 && line.chars().all(|c| matches!(c, '-' | '—' | '_'))
        })?;
        (fields.len() >= 3 && fields.contains("from") && has_address_or_time(&block.join(" ")))
            .then_some((separator, index + block.len()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIGNATURE: &str = "Joel Reed\nEngineering Lead, Example Co\n555-0100";

    fn history(texts: &[&str]) -> ThreadHistory {
        let mut history = ThreadHistory::default();
        texts.iter().for_each(|text| history.remember(text));
        history
    }

    fn new_text<'a>(history: &ThreadHistory, body: &'a str) -> &'a str {
        &body[..history.new_text_len(body)]
    }

    fn words(count: usize, prefix: &str) -> String {
        (0..count).map(|index| format!("{prefix}{index}")).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn limits_match_the_reader_policy() {
        let policy = include_str!("../../src/emailRenderingPolicy.ts");
        let start = policy.find("EMAIL_QUOTE_FOLDING_LIMITS = {").expect("quote folding limits");
        let block = &policy[start..start + policy[start..].find("} as const").unwrap()];
        let value = |name: &str| -> f64 {
            let line = block.lines().find(|line| line.trim_start().starts_with(&format!("{name}:"))).unwrap_or_else(|| panic!("{name} missing"));
            line.split(':').nth(1).unwrap().trim().trim_end_matches(',').parse().unwrap()
        };
        assert_eq!(value("shingleWords"), SHINGLE_WORDS as f64);
        assert_eq!(value("minSeenLineCoverage"), MIN_SEEN_LINE_COVERAGE);
        assert_eq!(value("minRepeatedRegionShingles"), MIN_REPEATED_REGION_SHINGLES as f64);
        assert_eq!(value("minCorroboratingShingles"), MIN_CORROBORATING_SHINGLES as f64);
        assert_eq!(value("minQuoteRunLines"), MIN_QUOTE_RUN_LINES as f64);
        assert_eq!(value("wrappedAttributionLookaheadLines"), WRAPPED_ATTRIBUTION_LOOKAHEAD_LINES as f64);
        assert_eq!(value("maxAttributionLength"), MAX_ATTRIBUTION_LENGTH as f64);
        assert_eq!(value("maxHeaderClusterLines"), MAX_HEADER_CLUSTER_LINES as f64);
    }

    #[test]
    fn removes_a_recorded_quote_with_its_attribution_and_repeated_signature() {
        let earlier = format!("Can you check the feed?\n\n{SIGNATURE}");
        let body = format!("Fixed now.\n\n{SIGNATURE}\n\nOn Mon, Oct 5, 2026 at 9:00 AM, A. Sender <a@example.com> wrote:\n> Can you check the feed?\n>\n> {}", SIGNATURE.replace('\n', "\n> "));
        assert_eq!(new_text(&history(&[&earlier]), &body), "Fixed now.\n\n");
    }

    #[test]
    fn keeps_a_quote_with_no_recorded_copy() {
        let body = "Fixed now.\n\nOn Mon, A wrote:\n> Is the feed fixed after the outage last week?\n> We saw errors again this morning around nine.";
        assert_eq!(new_text(&history(&["An unrelated earlier message about budgets."]), body), body);
        assert_eq!(new_text(&ThreadHistory::default(), body), body);
    }

    #[test]
    fn removes_only_the_recorded_part_of_a_partly_recorded_quote() {
        let recorded = words(12, "old");
        let body = format!("Reply.\n\nOn Mon, A wrote:\n> {}\n> {recorded}", words(6, "lost"));
        assert_eq!(new_text(&history(&[&recorded]), &body), format!("Reply.\n\nOn Mon, A wrote:\n> {}\n", words(6, "lost")));
    }

    #[test]
    fn keeps_recorded_text_followed_by_new_text() {
        let recorded = words(16, "old");
        let body = format!("Intro.\n{recorded}\nBut here is my new answer.");
        assert_eq!(new_text(&history(&[&recorded]), &body), body);
    }

    #[test]
    fn keeps_a_body_that_repeats_recorded_text_entirely() {
        let body = format!("Fixed now.\n\nOn Mon, A wrote:\n> {}", words(12, "old"));
        assert_eq!(new_text(&history(&[&body]), &body), body);
    }

    #[test]
    fn recognizes_wrapped_attributions_separators_and_header_blocks() {
        let recorded = words(8, "old");
        let bodies = [
            format!("Reply.\n\nOn Mon, Sep 21, 2026 at 2:33 PM A. Sender\n<a@example.com> wrote:\n\n> {recorded}"),
            format!("Reply.\n\n-----Original Message-----\n{recorded}"),
            format!("Reply.\n\n________________\nFrom: A <a@example.com>\nSent: Monday 9:00 AM\nSubject: Feed\n\n{recorded}"),
        ];
        for body in bodies {
            assert_eq!(new_text(&history(&[&recorded]), &body).trim_end(), "Reply.", "{body}");
        }
    }

    #[test]
    fn treats_crlf_bodies_like_lf_bodies() {
        let recorded = words(12, "old");
        let body = format!("Reply.\r\n\r\nOn Mon, A wrote:\r\n> {recorded}\r\n");
        assert_eq!(new_text(&history(&[&recorded]), &body), "Reply.\r\n\r\n");
    }

    #[test]
    fn standalone_repeated_text_needs_the_region_minimum() {
        for (copied, removed) in [
            (MIN_REPEATED_REGION_SHINGLES + SHINGLE_WORDS - 2, false),
            (MIN_REPEATED_REGION_SHINGLES + SHINGLE_WORDS - 1, true),
            (MIN_REPEATED_REGION_SHINGLES + SHINGLE_WORDS, true),
        ] {
            let recorded = words(copied, "old");
            let body = format!("Agreed, ship it.\n\n{recorded}");
            let expected = if removed { "Agreed, ship it.\n\n" } else { body.as_str() };
            assert_eq!(new_text(&history(&[&recorded]), &body), expected, "{copied} words");
        }
    }

    #[test]
    fn structural_quotes_and_signatures_need_the_corroborating_minimum() {
        for (quoted, removed) in [
            (MIN_CORROBORATING_SHINGLES + SHINGLE_WORDS - 2, false),
            (MIN_CORROBORATING_SHINGLES + SHINGLE_WORDS - 1, true),
            (MIN_CORROBORATING_SHINGLES + SHINGLE_WORDS, true),
        ] {
            let recorded = words(quoted, "old");
            let body = format!("Fixed.\n\nOn Mon, A wrote:\n> {recorded}");
            let expected = if removed { "Fixed.\n\n" } else { body.as_str() };
            assert_eq!(new_text(&history(&[&recorded]), &body), expected, "{quoted} quoted words");

            let signature = words(quoted, "sig");
            let quote = words(8, "old");
            let body = format!("Fixed.\n\n{signature}\n\nOn Mon, A wrote:\n> {quote}");
            let expected = if removed { "Fixed.\n\n".to_string() } else { format!("Fixed.\n\n{signature}\n\n") };
            assert_eq!(new_text(&history(&[&format!("{signature}\n{quote}")]), &body), expected, "{quoted} signature words");
        }
    }

    #[test]
    fn search_text_drops_repeated_quotes_but_keeps_every_word() {
        let original = "Can you check whether the nightly feed import still fails for the west region?";
        let reply = format!("Fixed now.\n\nOn Mon, A wrote:\n> {original}");
        // A quote with one word edited still counts as repeated (at least 80% covered).
        let edited = format!("Thanks!\n\nOn Tue, B wrote:\n> Fixed now.\n>\n> On Mon, A wrote:\n> > {}", original.replace("nightly", "hourly"));
        let text = searchable_thread_text([original, reply.as_str(), edited.as_str()]);
        assert_eq!(text.matches("nightly feed import").count(), 1);
        assert!(text.contains("Fixed now."));
        assert!(text.contains("Thanks!"));
        assert!(text.contains("hourly"), "{text}");
        assert!(!text.contains("On Mon, A wrote:\n> Can"), "{text}");
    }

    #[test]
    fn search_text_keeps_quotes_with_no_earlier_copy() {
        let reply = "Fixed now.\n\nOn Mon, A wrote:\n> The only copy of a message that is not stored here.";
        assert_eq!(searchable_thread_text([reply]), format!("{reply} "));
    }

    #[test]
    fn a_line_counts_as_repeated_at_the_coverage_limit() {
        assert_eq!(MIN_SEEN_LINE_COVERAGE, 0.8);
        let repeated = words(8, "old");
        // 8 recorded words beside 2 new ones is exactly 0.8 coverage.
        for (extra, removed) in [(1, true), (2, true), (3, false)] {
            let body = format!("Reply.\n\nOn Mon, A wrote:\n> {repeated} {}", words(extra, "new"));
            let kept = history(&[&repeated]).new_text_len(&body) == body.len();
            assert_eq!(!kept, removed, "{extra} new words");
        }
    }
}
