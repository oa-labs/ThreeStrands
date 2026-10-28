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

use std::borrow::Cow;
use std::collections::HashSet;

pub const SHINGLE_WORDS: usize = 4;
pub const MIN_SEEN_LINE_COVERAGE: f64 = 0.8;
pub const MIN_REPEATED_REGION_SHINGLES: usize = 8;
pub const MIN_CORROBORATING_SHINGLES: usize = 3;
pub const MIN_QUOTE_RUN_LINES: usize = 5;
pub const WRAPPED_ATTRIBUTION_LOOKAHEAD_LINES: usize = 3;
pub const MAX_ATTRIBUTION_LENGTH: usize = 500;
pub const MAX_HEADER_CLUSTER_LINES: usize = 12;
/// Characters of a search row's preview of the latest message, about the
/// length of a provider snippet.
pub const SEARCH_PREVIEW_CHARS: usize = 200;

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

    /// Byte length of `body` before any quoted history, for a preview: the
    /// shorter of [`Self::new_text_len`] and a structural boundary (an
    /// attribution, `>` run or header block) with text above it. Unlike
    /// `new_text_len` this also cuts at a quote with no recorded copy, so it
    /// suits text that is always kept in full somewhere else.
    pub fn preview_text_len(&self, body: &str) -> usize {
        let new_len = self.new_text_len(body);
        let lines = split_lines(body);
        let texts: Vec<&str> = lines.iter().map(|(_, text)| *text).collect();
        structural_cut(&texts)
            .filter(|(cut, _)| texts[..*cut].iter().any(|text| !line_words(text).is_empty()))
            .map_or(new_len, |(cut, _)| new_len.min(lines[cut].0))
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

/// What a thread's search row indexes from its message bodies.
pub struct ThreadSearchText {
    /// Each body minus a trailing region that repeats an earlier body in the
    /// same thread. The earliest copy of any text is never removed — nothing
    /// precedes it — so the row loses no phrase it held before. Words in a
    /// removed region that the thread has not used yet (a line counts as
    /// repeated at `MIN_SEEN_LINE_COVERAGE`, not 100%) are kept, so every
    /// word stays searchable.
    pub body: String,
    /// The latest message before any quoted history (or, for a forward with
    /// no comment, the forwarded message), cleaned for one line by
    /// [`crate::preview_text::message_preview`]; empty when that message has
    /// no text. Everything here is also in `body`, so cutting at an
    /// unconfirmed quote loses no matches.
    pub latest_preview: String,
}

/// Search text for a thread's message bodies, oldest first.
pub fn searchable_thread_text<'a>(bodies: impl IntoIterator<Item = &'a str>) -> ThreadSearchText {
    searchable_thread_text_with(bodies, &crate::preview_text::PreviewContext::default())
}

/// [`searchable_thread_text`], with what the caller knows about the latest
/// message beyond its plain text to shape its preview.
pub fn searchable_thread_text_with<'a>(
    bodies: impl IntoIterator<Item = &'a str>,
    context: &crate::preview_text::PreviewContext,
) -> ThreadSearchText {
    let mut history = ThreadHistory::default();
    let mut used: HashSet<String> = HashSet::new();
    let mut out = String::new();
    let mut latest = Cow::Borrowed("");
    for body in bodies {
        latest = preview_source(&history, body);
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
    ThreadSearchText { body: out, latest_preview: crate::preview_text::message_preview(&latest, context) }
}

/// Most lines, and most words on each, the text above a forwarded message
/// may have to count as a signature rather than a comment.
pub const MAX_SIGNATURE_LINES: usize = 8;
pub const MAX_SIGNATURE_LINE_WORDS: usize = 6;

/// The part of `body` a preview shows: its text before any quoted history,
/// or the forwarded message when nothing but a signature precedes it.
fn preview_source<'a>(history: &ThreadHistory, body: &'a str) -> Cow<'a, str> {
    let cut = history.preview_text_len(body);
    forwarded_text(body, cut).map_or(Cow::Borrowed(&body[..cut]), Cow::Owned)
}

/// The forwarded message's own text, before any history it quotes, when a
/// forwarded-message separator at or before `cut` has only a signature (or
/// nothing) above it. A forwarded message quoted with `>` loses one level.
fn forwarded_text(body: &str, cut: usize) -> Option<String> {
    let lines = split_lines(body);
    let separator = lines.iter().position(|(_, text)| {
        is_separator_marker(text) && text.to_lowercase().contains("forward")
    })?;
    if lines[separator].0 > cut {
        return None;
    }
    let above: Vec<&str> = lines[..separator].iter().map(|(_, text)| *text).filter(|text| !text.trim().is_empty()).collect();
    if !is_signature(&above) {
        return None;
    }
    // Past the forwarded header block: its fields up to the first blank line.
    let mut index = separator + 1;
    while lines.get(index).is_some_and(|(_, text)| text.trim().is_empty()) {
        index += 1;
    }
    if lines.get(index).is_some_and(|(_, text)| header_field(text).is_some()) {
        while lines.get(index).is_some_and(|(_, text)| !text.trim().is_empty()) {
            index += 1;
        }
    }
    let rest = &body[lines.get(index).map_or(body.len(), |(start, _)| *start)..];
    let quoted = rest
        .lines()
        .find(|line| !line_words(line).is_empty())
        .is_some_and(|line| line.trim_start().starts_with('>'));
    let rest = if quoted {
        rest.lines()
            .map(|line| {
                let line = line.trim_start();
                line.strip_prefix('>').map_or(line, |unquoted| unquoted.strip_prefix(' ').unwrap_or(unquoted))
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        rest.to_string()
    };
    let forwarded = &rest[..ThreadHistory::default().preview_text_len(&rest)];
    forwarded.lines().any(|line| !line_words(line).is_empty()).then(|| forwarded.to_string())
}

/// Whether `lines` read as a sign-off rather than a comment: few short lines,
/// no question or exclamation, and a contact detail (an address, a phone
/// number, or a `|` between title parts). No lines at all also qualifies.
fn is_signature(lines: &[&str]) -> bool {
    if lines.is_empty() {
        return true;
    }
    let contact = |line: &str| {
        line.contains('@') || line.contains('|') || line.chars().filter(char::is_ascii_digit).count() >= 7
    };
    lines.len() <= MAX_SIGNATURE_LINES
        && lines.iter().all(|line| {
            line_words(line).len() <= MAX_SIGNATURE_LINE_WORDS && !line.trim_end().ends_with(['?', '!'])
        })
        && lines.iter().any(|line| contact(line))
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
    // `get` rather than indexing: a line may start with a multibyte character.
    trimmed.get(..3).is_some_and(|prefix| prefix.eq_ignore_ascii_case("on "))
        && trimmed.get(3..).and_then(|rest| rest.chars().next()).is_some_and(|c| !c.is_whitespace())
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
        // The address may be bracketed however a converter wrote it:
        // `<a@b>`, or `[a@b]>` where a link label landed inside the brackets.
        let is_continuation = rest.is_some_and(|rest| {
            rest.is_empty() || (rest.contains('@') && !rest.contains(char::is_whitespace))
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
/// The earliest of an attribution or separator, a `>` run, and a header
/// block wins.
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
    [marker, quote_run, header_block(lines)].into_iter().flatten().min_by_key(|(cut, _)| *cut)
}

/// A From/Sent/To/Subject block starting at a header line after other text.
/// After a separator line the cut starts at the separator; without one the
/// block also needs a Sent or Date and a To or Subject, and the cut starts
/// at the block. Either way it needs three fields including From, and an
/// address or a time.
fn header_block(lines: &[&str]) -> Option<(usize, usize)> {
    (1..lines.len()).find_map(|index| {
        header_field(lines[index])?;
        let block: Vec<&str> = lines[index..]
            .iter()
            .take_while(|line| !line.trim().is_empty())
            .take(MAX_HEADER_CLUSTER_LINES)
            .copied()
            .collect();
        let fields: HashSet<&str> = block.iter().filter_map(|line| header_field(line)).collect();
        if !(fields.len() >= 3 && fields.contains("from") && has_address_or_time(&block.join(" "))) {
            return None;
        }
        let end = index + block.len();
        let previous = (0..index).rev().find(|&line| !lines[line].trim().is_empty())?;
        let line = lines[previous].trim();
        if line.chars().count() >= 2 && line.chars().all(|c| matches!(c, '-' | '—' | '_')) {
            return Some((previous, end));
        }
        let dated = fields.contains("sent") || fields.contains("date");
        let addressed = fields.contains("to") || fields.contains("subject");
        (dated && addressed && header_field(lines[previous]).is_none()).then_some((index, end))
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
        let policy = include_str!("../../../src/emailRenderingPolicy.ts");
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
            // A link label wrapped onto the line with "wrote:".
            format!("Reply.\n\nOn Tue, September 15, 2026 1:00 PM, A. Sender <a@example.com>\n[a@example.com]> wrote:\n\n> {recorded}"),
            // A header block with no separator line above it.
            format!("Reply.\nFrom: A <a@example.com>\nSent: Monday, September 14, 2026 at 10:40 AM\nTo: b@example.com\nSubject: Feed\n{recorded}"),
        ];
        for body in bodies {
            assert_eq!(new_text(&history(&[&recorded]), &body).trim_end(), "Reply.", "{body}");
        }
    }

    #[test]
    fn handles_lines_that_start_with_multibyte_characters() {
        let recorded = "日本語のメッセージ 本文 です 確認 お願い します 今日 明日 来週";
        for line in ["é", "éé", "日本", "🙂 ok", "Oñ Monday"] {
            let body = format!("{line}\n\nOn Mon, A wrote:\n> {recorded}");
            assert!(history(&[recorded]).new_text_len(&body) <= body.len(), "{line}");
            assert!(history(&[recorded]).preview_text_len(&body) <= body.len(), "{line}");
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
        let text = searchable_thread_text([original, reply.as_str(), edited.as_str()]).body;
        assert_eq!(text.matches("nightly feed import").count(), 1);
        assert!(text.contains("Fixed now."));
        assert!(text.contains("Thanks!"));
        assert!(text.contains("hourly"), "{text}");
        assert!(!text.contains("On Mon, A wrote:\n> Can"), "{text}");
    }

    #[test]
    fn search_text_keeps_quotes_with_no_earlier_copy() {
        let reply = "Fixed now.\n\nOn Mon, A wrote:\n> The only copy of a message that is not stored here.";
        assert_eq!(searchable_thread_text([reply]).body, format!("{reply} "));
    }

    #[test]
    fn search_preview_is_the_latest_message_before_any_quote() {
        let original = "Can you check whether the nightly feed import still fails?";
        let reply = format!("Fixed   now,\nthe hourly retry handles it.\n\nOn Mon, A wrote:\n> {original}");
        assert_eq!(searchable_thread_text([original, reply.as_str()]).latest_preview, "Fixed now, the hourly retry handles it.");
        // A quote with no earlier copy stays in `body` but not in the preview.
        let lone = "Thanks!\n\nOn Mon, A wrote:\n> The only copy of a message that is not stored here.";
        let text = searchable_thread_text([lone]);
        assert_eq!(text.latest_preview, "Thanks!");
        assert!(text.body.contains("The only copy"));
        // A body that is only a quote keeps it, so the preview is never empty for a message with text.
        assert_eq!(searchable_thread_text(["On Mon, A wrote:\n> hello"]).latest_preview, "On Mon, A wrote: > hello");
        assert_eq!(searchable_thread_text([original, ""]).latest_preview, "");
    }

    #[test]
    fn search_preview_drops_markdown_and_urls_but_body_keeps_them() {
        let body = "Hey Joel I'm [Amit](https://www.example.com/in/amit/) and **this** is the list:\nhttps://example.com/list?id=1";
        let text = searchable_thread_text([body]);
        assert_eq!(text.latest_preview, "Hey Joel I'm Amit and this is the list:");
        assert!(text.body.contains("https://example.com/list?id=1"));
        // Nothing but a link still previews as the link.
        assert_eq!(searchable_thread_text(["https://example.com/x"]).latest_preview, "https://example.com/x");
    }

    #[test]
    fn search_preview_is_capped_at_the_preview_length() {
        for (length, kept) in [
            (SEARCH_PREVIEW_CHARS - 1, SEARCH_PREVIEW_CHARS - 1),
            (SEARCH_PREVIEW_CHARS, SEARCH_PREVIEW_CHARS),
            (SEARCH_PREVIEW_CHARS + 1, SEARCH_PREVIEW_CHARS),
        ] {
            let body = "é".repeat(length);
            assert_eq!(searchable_thread_text([body.as_str()]).latest_preview.chars().count(), kept);
        }
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

    fn preview(bodies: &[&str]) -> String {
        searchable_thread_text(bodies.iter().copied()).latest_preview
    }

    #[test]
    fn the_earliest_quote_structure_ends_the_preview() {
        // A header block above an older "On … wrote:" further down.
        let body = "See you then!\nLisa\n________________________________\nFrom: Joel Reed <joel@example.com>\nSent: Thursday, 17 September 2026 13:25:16\nTo: Lisa <lisa@example.com>\nSubject: Re: See you soon\n\nThank you Lisa.\n\nOn 2026-09-17T17:20:42+00:00, Lisa <lisa@example.com> wrote:\n> Thanks for the treat!";
        assert_eq!(preview(&[body]), "See you then! Lisa");
        // A `>` run above a later separator.
        let body = "Agreed.\n> one\n> two\n> three\n> four\n> five\n\n-----Original Message-----\nolder";
        assert_eq!(preview(&[body]), "Agreed.");
        // An attribution wrapped with a bracketed address.
        let body = "Half an hour, I promise.\n\nOn Tue, September 15, 2026 1:00 PM, Colin <c@example.com>\n[c@example.com]> wrote:\n\n> Joel,\n> Interested in coming on as a guest?";
        assert_eq!(preview(&[body]), "Half an hour, I promise.");
    }

    #[test]
    fn a_header_block_without_a_separator_needs_a_date_and_an_addressee() {
        let block = |fields: &str| format!("Following up on the below.\n{fields}\n\nJoel, in light of your success I am writing to you.");
        for (fields, cut) in [
            ("From: A <a@example.com>\nSent: Monday, September 14, 2026 at 10:40 AM\nTo: joel@example.com", true),
            ("From: A <a@example.com>\nDate: Mon, Sep 14, 2026 at 10:40 AM\nSubject: Intro", true),
            // Three fields, but no date or no addressee.
            ("From: A <a@example.com>\nTo: joel@example.com\nCc: b@example.com", false),
            ("From: A <a@example.com>\nSent: Monday 10:40 AM\nCc: b@example.com", false),
            // A date and an addressee, but only two fields.
            ("From: A <a@example.com>\nSent: Monday 10:40 AM", false),
        ] {
            assert_eq!(preview(&[&block(fields)]) == "Following up on the below.", cut, "{fields}");
        }
    }

    #[test]
    fn a_forward_with_only_a_signature_previews_the_forwarded_message() {
        assert_eq!((MAX_SIGNATURE_LINES, MAX_SIGNATURE_LINE_WORDS), (8, 6));
        let forwarded = "---------- Forwarded message ---------\nFrom: Andrew <andrew@example.com>\nDate: Thu, Aug 27, 2026, 1:27 PM\nSubject: Hiring report\nTo: Cara <cara@example.com>, Everyone <\neveryone@example.com>\n\n\nTeam, here's a competitors report.\n\nOn Wed, Cara <cara@example.com> wrote:\n> Can someone pull the report?";
        for above in [
            "Joel Reed\nCEO | OpenArc\n412.212.8417\n\nHelping clients build great software\nand hire great people.\n\n",
            "Ann Lee\nann@example.com\n",
            "",
        ] {
            assert_eq!(preview(&[&format!("{above}{forwarded}")]), "Team, here's a competitors report.", "{above}");
        }
        for above in [
            // A comment: a question, a long line, or no contact detail.
            "Can you handle this?\nAnn | 555-010-0000\n\n",
            "Here is the report our clients were so impressed by last week.\nAnn | 555-010-0000\n\n",
            "Please review.\nAnn\n\n",
        ] {
            let text = preview(&[&format!("{above}{forwarded}")]);
            assert!(text.starts_with(&above.split('\n').next().unwrap()[..6]), "{above} → {text}");
        }
        // Signature lines at, and past, their limits.
        let lines = |count: usize, words: usize| (0..count).map(|_| vec!["w"; words].join(" ")).collect::<Vec<_>>().join("\n") + "\n555-010-0000\n\n";
        for (above, forwarded_shown) in [
            (lines(MAX_SIGNATURE_LINES - 1, MAX_SIGNATURE_LINE_WORDS), true),
            (lines(MAX_SIGNATURE_LINES, MAX_SIGNATURE_LINE_WORDS), false),
            (lines(1, MAX_SIGNATURE_LINE_WORDS + 1), false),
        ] {
            assert_eq!(preview(&[&format!("{above}{forwarded}")]).starts_with("Team"), forwarded_shown, "{above}");
        }
        // A forwarded message quoted with `>` loses one level, and history it quotes is cut.
        let quoted = "---------- Forwarded message ----------\nFrom: Shop <hello@example.com>\nDate: Mar 11, 2026 at 6:00 PM\n\n>\n> SHOP\n> Pack your bags.\n>\n> On Mon, A <a@example.com> wrote:\n> > older";
        assert_eq!(preview(&[quoted]), "SHOP Pack your bags.");
        // A forward under an earlier quote cut is not reached.
        let body = format!("Thanks!\n\nOn Mon, A <a@example.com> wrote:\n> Joel Reed\n> 412.212.8417\n{forwarded}");
        assert_eq!(preview(&[&body]), "Thanks!");
    }
}
