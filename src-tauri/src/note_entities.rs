//! Provider-neutral, inert Markdown references and round tasks. All offsets
//! are UTF-8 byte offsets into the original body; patches never reserialize it.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use threestrands_sync_protocol::{MAX_NOTE_ENTITY_LINKS, MAX_NOTE_REFERENCE_BYTES, MAX_NOTE_TASKS};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NoteReference {
    pub version: u8,
    pub kind: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// A contact reference's saved contact id, when there was one. The
    /// reference's `id` is the person's address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
    /// An event occurrence's recurring series, when it has one, so the link
    /// also counts toward every note about that series.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
}
/// An address as contact lookup stores it: trimmed and ASCII-lowercased.
pub fn normalize_contact_address(address: &str) -> String {
    address.trim().to_ascii_lowercase()
}
fn is_contact_address(id: &str) -> bool {
    let mut parts = id.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !id.chars().any(char::is_whitespace)
        && normalize_contact_address(id) == id
}
impl NoteReference {
    pub fn validate(&self) -> bool {
        let valid = |v: &str| !v.is_empty() && !v.chars().any(char::is_control);
        self.version == 1
            && valid(&self.id)
            && self.account.as_deref().is_none_or(valid)
            && self.calendar.as_deref().is_none_or(valid)
            && self.note.as_deref().is_none_or(valid)
            && self.contact.as_deref().is_none_or(valid)
            && self.series.as_deref().is_none_or(valid)
            && (self.kind == "contact" || self.contact.is_none())
            && (self.kind == "event" || self.series.is_none())
            && self.series.as_deref() != Some(self.id.as_str())
            && match self.kind.as_str() {
                "conversation" => {
                    self.account.is_some() && self.calendar.is_none() && self.note.is_none()
                }
                "task" => self.account.is_none() && self.calendar.is_none(),
                "event" => self.account.is_some() && self.calendar.is_some() && self.note.is_none(),
                "contact" => {
                    is_contact_address(&self.id)
                        && self.account.is_none()
                        && self.calendar.is_none()
                        && self.note.is_none()
                }
                _ => false,
            }
    }
    /// The target this reference names, as the link index keys it. A contact
    /// reference names its address, so links written with and without a
    /// contact id, or before and after a merge, are one target; an event
    /// occurrence names itself, with or without its series.
    pub fn target(&self) -> Result<String, String> {
        Self {
            contact: None,
            series: None,
            ..self.clone()
        }
        .href()
    }
    /// The target of the recurring series an event occurrence belongs to:
    /// the same target a link to the entire series has.
    pub fn series_target(&self) -> Result<Option<String>, String> {
        match (&self.series, self.kind.as_str()) {
            (Some(series), "event") => Self {
                id: series.clone(),
                series: None,
                ..self.clone()
            }
            .href()
            .map(Some),
            _ => Ok(None),
        }
    }
    pub fn href(&self) -> Result<String, String> {
        if !self.validate() {
            return Err("Invalid note reference".into());
        }
        let encoded = percent_encoding::utf8_percent_encode(
            &serde_json::to_string(self).map_err(|e| e.to_string())?,
            percent_encoding::NON_ALPHANUMERIC,
        )
        .to_string();
        let href = format!("threestrands:{}/{encoded}", self.kind);
        if href.len() > MAX_NOTE_REFERENCE_BYTES {
            return Err("This reference is too long".into());
        }
        Ok(href)
    }
    pub fn parse(href: &str) -> Option<Self> {
        if href.len() > MAX_NOTE_REFERENCE_BYTES {
            return None;
        }
        let (kind, payload) = href.strip_prefix("threestrands:")?.split_once('/')?;
        // Refuse malformed percent escapes rather than accepting a different payload.
        let bytes = payload.as_bytes();
        for (i, b) in bytes.iter().enumerate() {
            if *b == b'%'
                && (i + 2 >= bytes.len()
                    || !bytes[i + 1].is_ascii_hexdigit()
                    || !bytes[i + 2].is_ascii_hexdigit())
            {
                return None;
            }
        }
        let decoded = percent_encoding::percent_decode_str(payload)
            .decode_utf8()
            .ok()?;
        let json: serde_json::Value = serde_json::from_str(&decoded).ok()?;
        if ["account", "calendar", "note", "contact", "series"]
            .iter()
            .any(|key| json.get(key).is_some_and(serde_json::Value::is_null))
        {
            return None;
        }
        let value: Self = serde_json::from_value(json).ok()?;
        (value.kind == kind && value.validate()).then_some(value)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteItem {
    pub id: String,
    pub title: String,
    pub checked: bool,
    pub task: bool,
    pub start: usize,
    pub marker_start: usize,
    pub marker_end: usize,
    pub end: usize,
    pub persisted: bool,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityLink {
    pub reference: NoteReference,
    pub href: String,
    pub label: String,
    pub context: String,
    pub start: usize,
}
#[derive(Default, Debug)]
pub struct ParsedNote {
    pub items: Vec<NoteItem>,
    pub links: Vec<EntityLink>,
    pub unclosed_fence: bool,
}
const ANCHOR: &str = "<!-- threestrands-task:";
fn anchor(line: &str) -> Option<(&str, usize)> {
    let start = line.rfind(ANCHOR)?;
    let id = line[start + ANCHOR.len()..].strip_suffix(" -->")?;
    if id.len() != 36 {
        return None;
    }
    uuid::Uuid::parse_str(id).ok()?;
    Some((id, start))
}
fn derived_id(note: &str, text: &str, occurrence: usize) -> String {
    let digest = Sha256::digest(format!("note-task-v1\0{note}\0{text}\0{occurrence}").as_bytes());
    let mut bytes: [u8; 16] = digest[..16].try_into().unwrap();
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}
fn label_ends(line: &str) -> HashMap<usize, usize> {
    let bytes = line.as_bytes();
    let mut ends = HashMap::new();
    let mut stack = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'[' => stack.push(i),
            b']' => {
                if let Some(start) = stack.pop() {
                    ends.insert(start, i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    ends
}

fn list_prefix(line: &str) -> Option<usize> {
    let rest = line.trim_start_matches([' ', '\t']);
    let indent = line.len() - rest.len();
    let bytes = rest.as_bytes();
    let n = if matches!(bytes.first(), Some(b'-' | b'+' | b'*')) {
        1
    } else {
        let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
        if digits == 0 || !matches!(bytes.get(digits), Some(b'.' | b')')) {
            return None;
        }
        digits + 1
    };
    if !matches!(bytes.get(n), Some(b' ' | b'\t')) {
        return None;
    }
    Some(indent + n + rest[n..].len() - rest[n..].trim_start_matches([' ', '\t']).len())
}
/// Recognizes list markers only in prose. Fenced code, indented standalone
/// code, blockquotes, inline code, comments and escaped links are inert.
pub fn parse(note: &str, body: &str) -> Result<ParsedNote, String> {
    let mut out = ParsedNote::default();
    let mut fence: Option<(u8, usize)> = None;
    let mut html_comment = false;
    let mut quote_fence: Option<(u8, usize)> = None;
    let mut occurrence: HashMap<String, usize> = HashMap::new();
    let mut used = HashSet::new();
    let mut offset = 0;
    let mut list_indent: Option<usize> = None;
    for piece in body.split_inclusive('\n') {
        let line = piece.trim_end_matches(['\r', '\n']);
        let text = line.trim_start_matches([' ', '\t']);
        let indent =
            line.chars()
                .take_while(|ch| matches!(ch, ' ' | '\t'))
                .fold(0, |column, ch| {
                    if ch == '\t' {
                        column + 4 - column % 4
                    } else {
                        column + 1
                    }
                });
        let mark = text.as_bytes().first().copied().unwrap_or(0);
        let run = text.bytes().take_while(|b| *b == mark).count();
        let inside = fence.is_some();
        if matches!(mark, b'`' | b'~')
            && run >= 3
            && (fence.is_some() || indent <= 3 || list_indent.is_some_and(|parent| indent > parent))
        {
            match fence {
                None => fence = Some((mark, run)),
                Some((ch, n)) if ch == mark && run >= n && text[run..].trim().is_empty() => {
                    fence = None
                }
                _ => {}
            }
            offset += piece.len();
            continue;
        }
        if inside {
            offset += piece.len();
            continue;
        }
        let quoted = text.starts_with('>');
        if quoted {
            list_indent = None;
            let mut content = text;
            while let Some(rest) = content.strip_prefix('>') {
                content = rest.strip_prefix(' ').unwrap_or(rest);
            }
            let mark = content.as_bytes().first().copied().unwrap_or(0);
            let run = content.bytes().take_while(|b| *b == mark).count();
            if matches!(mark, b'`' | b'~') && run >= 3 {
                match quote_fence {
                    None => quote_fence = Some((mark, run)),
                    Some((ch, n)) if ch == mark && run >= n && content[run..].trim().is_empty() => {
                        quote_fence = None
                    }
                    _ => {}
                }
                offset += piece.len();
                continue;
            }
            if quote_fence.is_some() || content.starts_with("    ") || content.starts_with('\t') {
                offset += piece.len();
                continue;
            }
        } else {
            quote_fence = None;
        }
        let prefix = list_prefix(line);
        let standalone_code = indent >= 4 && list_indent.is_none_or(|parent| indent <= parent);
        if !text.is_empty() && indent == 0 && prefix.is_none() {
            list_indent = None;
        }
        if standalone_code {
            offset += piece.len();
            continue;
        }
        if prefix.is_some() {
            list_indent = Some(indent);
        }
        if !html_comment && !quoted {
            if let Some(at) = prefix {
                let rest = &line[at..];
                let family = rest.as_bytes().first().copied().unwrap_or(0);
                if matches!(family, b'(' | b'[') {
                    let close = if family == b'(' { ')' } else { ']' };
                    if let Some(n) = rest.find(close) {
                        let state = &rest[1..n];
                        if matches!(state, "" | " " | "x" | "X")
                            && rest[n + 1..].starts_with([' ', '\t'])
                        {
                            let source = rest[n + 1..].trim();
                            let found = anchor(line);
                            let title_source =
                                found.map_or(source, |(_, end)| line[at + n + 1..end].trim());
                            if !title_source.is_empty() {
                                let count = occurrence.entry(title_source.to_string()).or_default();
                                let mut id = found
                                    .map(|(id, _)| id.to_string())
                                    .unwrap_or_else(|| derived_id(note, title_source, *count));
                                *count += 1;
                                let mut persisted = found.is_some();
                                if !used.insert(id.clone()) {
                                    id = derived_id(
                                        note,
                                        &format!("duplicate:{id}:{title_source}"),
                                        *count,
                                    );
                                    persisted = false;
                                    used.insert(id.clone());
                                }
                                out.items.push(NoteItem {
                                    id,
                                    title: crate::notes::inline_plain(title_source)
                                        .split_whitespace()
                                        .collect::<Vec<_>>()
                                        .join(" "),
                                    checked: matches!(state, "x" | "X"),
                                    task: family == b'(',
                                    start: offset,
                                    marker_start: offset + at,
                                    marker_end: offset + at + n + 1,
                                    end: offset + line.len(),
                                    persisted,
                                });
                            }
                        }
                    }
                }
            }
        }
        let labels = label_ends(line);
        let mut context = None;
        let mut i = 0;
        let bytes = line.as_bytes();
        while i < bytes.len() {
            if html_comment {
                if let Some(end) = line[i..].find("-->") {
                    i += end + 3;
                    html_comment = false;
                } else {
                    break;
                }
                continue;
            }
            if line[i..].starts_with("<!--") {
                html_comment = true;
                i += 4;
                continue;
            }
            if bytes[i] == b'\\' {
                i += 1;
                if i < bytes.len() {
                    i += line[i..].chars().next().unwrap().len_utf8();
                }
                continue;
            }
            if bytes[i] == b'`' {
                let n = bytes[i..].iter().take_while(|b| **b == b'`').count();
                let mut j = i + n;
                let mut closed = false;
                while j < bytes.len() {
                    if bytes[j] == b'`' {
                        let k = bytes[j..].iter().take_while(|b| **b == b'`').count();
                        if k == n {
                            j += k;
                            closed = true;
                            break;
                        }
                        j += k;
                    } else {
                        j += 1;
                    }
                }
                i = if closed { j } else { i + n };
                continue;
            }
            if bytes[i] == b'[' && (i == 0 || bytes[i - 1] != b'!') {
                if let Some(&at) = labels.get(&i) {
                    if bytes.get(at + 1) != Some(&b'(') {
                        i += 1;
                        continue;
                    }
                    if let Some(end) = line[at + 2..].find(')') {
                        let href = &line[at + 2..at + 2 + end];
                        if href.starts_with("threestrands:")
                            && href.len() > MAX_NOTE_REFERENCE_BYTES
                        {
                            return Err(
                                "This note contains an item reference that is too long".into()
                            );
                        }
                        if let Some(reference) = NoteReference::parse(href) {
                            out.links.push(EntityLink {
                                href: reference.href()?,
                                reference,
                                label: crate::notes::inline_plain(&line[i + 1..at]),
                                context: context
                                    .get_or_insert_with(|| {
                                        crate::notes::line_context(&crate::notes::plain_text(line))
                                    })
                                    .clone(),
                                start: offset + i,
                            });
                        }
                        i = at + end + 3;
                        continue;
                    }
                }
            }
            i += line[i..].chars().next().unwrap().len_utf8();
        }
        offset += piece.len();
    }
    out.unclosed_fence = fence.is_some();
    if out.items.iter().filter(|item| item.task).count() > MAX_NOTE_TASKS {
        return Err("This note has too many tasks (maximum 500)".into());
    }
    if out.links.len() > MAX_NOTE_ENTITY_LINKS {
        return Err("This note has too many item links (maximum 500)".into());
    }
    Ok(out)
}
/// Only called for authored edits/imports, never projection or reads.
pub fn normalize(note: &str, body: &str) -> Result<String, String> {
    if threestrands_sync_protocol::note_body_bytes(body)
        > threestrands_sync_protocol::MAX_NOTE_BODY_BYTES
    {
        return Err("This note is too long to sync. Shorten it to save your changes.".into());
    }
    let parsed = parse(note, body)?;
    let mut body = body.to_string();
    for item in parsed
        .items
        .iter()
        .rev()
        .filter(|item| item.task && !item.persisted)
    {
        let line = &body[item.start..item.end];
        let replace = anchor(line).map(|(_, at)| item.start + at);
        match replace {
            Some(at) => body.replace_range(at..item.end, &format!("{ANCHOR}{} -->", item.id)),
            None => body.insert_str(item.end, &format!(" {ANCHOR}{} -->", item.id)),
        }
    }
    Ok(body)
}
pub fn patch(
    note: &str,
    body: &str,
    id: &str,
    checked: Option<bool>,
    task: Option<bool>,
) -> Result<String, String> {
    let parsed = parse(note, body)?;
    let item = parsed
        .items
        .iter()
        .find(|item| item.id == id)
        .ok_or("This item changed. Open its note to try again.")?;
    let family = task.unwrap_or(item.task);
    let state = if checked.unwrap_or(item.checked) {
        "x"
    } else {
        " "
    };
    let mut result = body.to_string();
    if !item.persisted && (item.task || family) {
        result.insert_str(item.end, &format!(" {ANCHOR}{} -->", item.id));
    }
    result.replace_range(
        item.marker_start..item.marker_end,
        &if family {
            format!("({state})")
        } else {
            format!("[{state}]")
        },
    );
    normalize(note, &result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_frontend_parser_fixtures() {
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("../../fixtures/note-entities.json")).unwrap();
        for fixture in fixtures.as_array().unwrap() {
            let parsed = parse("n", fixture["body"].as_str().unwrap()).unwrap();
            assert_eq!(
                serde_json::json!(parsed.items.iter().map(|i| i.task).collect::<Vec<_>>()),
                fixture["kinds"],
                "{}",
                fixture["name"]
            );
            assert_eq!(
                serde_json::json!(parsed.items.iter().map(|i| i.checked).collect::<Vec<_>>()),
                fixture["checked"],
                "{}",
                fixture["name"]
            );
        }
    }
    #[test]
    fn tasks_and_checklists_are_distinct_and_only_real_prose_is_indexed() {
        let body="- [ ] milk\n- [x] bread\n- () send\n1. (X) done\n  - ( ) nested\n> - ( ) quote\n    - ( ) code\n```md\n- ( ) fenced\n```\n`- ( ) inline`\n- \\( ) escaped\n- ( ) \n<!--\n- ( ) comment\n-->\n- [label](https://example.com)\n";
        let p = parse("n", body).unwrap();
        assert_eq!(
            p.items
                .iter()
                .map(|i| (&*i.title, i.task, i.checked))
                .collect::<Vec<_>>(),
            [
                ("milk", false, false),
                ("bread", false, true),
                ("send", true, false),
                ("done", true, true),
                ("nested", true, false)
            ]
        );
    }
    #[test]
    fn edits_preserve_surroundings_and_identity_across_conversion() {
        let body = "# Plan\r\n\r\n1. () **Send** [[Plan]]\r\n   - [ ] child\r\n";
        let normalized = normalize("n", body).unwrap();
        let i = parse("n", &normalized).unwrap().items.remove(0);
        assert!(i.persisted);
        assert_eq!(
            parse("n", "- ( ) 1. **Send** mail").unwrap().items[0].title,
            "1. Send mail"
        );
        let checked = patch("n", &normalized, &i.id, Some(true), None).unwrap();
        assert!(checked.starts_with("# Plan\r\n\r\n1. (x) **Send** [[Plan]]"));
        assert!(checked.ends_with("\r\n   - [ ] child\r\n"));
        let square = patch("n", &checked, &i.id, None, Some(false)).unwrap();
        let item = parse("n", &square).unwrap().items.remove(0);
        assert_eq!(item.id, i.id);
        assert!(!item.task);
        assert!(item.checked);
        let round = patch("n", &square, &i.id, None, Some(true)).unwrap();
        assert_eq!(parse("n", &round).unwrap().items[0].id, i.id);
        assert_eq!(normalize("n", &round).unwrap(), round);
    }
    #[test]
    fn provisional_ids_agree_on_two_devices_and_duplicate_markers_are_repaired() {
        let a = normalize("n", "- ( ) Same\n- ( ) Same\n").unwrap();
        assert_eq!(normalize("n", "- ( ) Same\n- ( ) Same\n").unwrap(), a);
        let p = parse("n", &a).unwrap();
        assert_ne!(p.items[0].id, p.items[1].id);
        let copied = format!("{a}{}\n", a.lines().next().unwrap());
        let repaired = normalize("n", &copied).unwrap();
        let ids: HashSet<_> = parse("n", &repaired)
            .unwrap()
            .items
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(normalize("n", &repaired).unwrap(), repaired);
        assert_eq!(
            normalize("n", "- [ ] shopping\n").unwrap(),
            "- [ ] shopping\n"
        );
    }
    fn reference() -> NoteReference {
        NoteReference {
            version: 1,
            kind: "conversation".into(),
            id: "邮件/a(b)".into(),
            account: Some("you@example.com".into()),
            calendar: None,
            note: None,
            contact: None,
            series: None,
        }
    }
    #[test]
    fn internal_references_are_strict_inert_and_round_trip_unicode() {
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("../../fixtures/note-references.json")).unwrap();
        for fixture in fixtures.as_array().unwrap() {
            let reference: NoteReference =
                serde_json::from_value(fixture["reference"].clone()).unwrap();
            assert_eq!(reference.href().unwrap(), fixture["href"].as_str().unwrap());
            assert_eq!(
                NoteReference::parse(fixture["href"].as_str().unwrap()),
                Some(reference)
            );
        }
        let r = reference();
        let href = r.href().unwrap();
        assert_eq!(NoteReference::parse(&href), Some(r));
        for invalid in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "threestrands:conversation/%xx",
            "threestrands:run/%7B%7D",
        ] {
            assert!(NoteReference::parse(invalid).is_none());
        }
        let json = serde_json::json!({"version":1,"kind":"task","id":"a","command":"run"});
        assert!(NoteReference::parse(&format!(
            "threestrands:task/{}",
            percent_encoding::utf8_percent_encode(
                &json.to_string(),
                percent_encoding::NON_ALPHANUMERIC
            )
        ))
        .is_none());
        let body=format!("[One]({href})\n`[Code]({href})`\n\\[Escape]({href})\n```\n[Block]({href})\n```\n<!-- [hidden]({href}) -->\n[Two]({href})");
        let links = parse("n", &body).unwrap().links;
        assert_eq!(
            links.iter().map(|l| l.label.as_str()).collect::<Vec<_>>(),
            ["One", "Two"]
        );
    }
    fn contact(id: &str, contact: Option<&str>) -> NoteReference {
        NoteReference {
            version: 1,
            kind: "contact".into(),
            id: id.into(),
            account: None,
            calendar: None,
            note: None,
            contact: contact.map(Into::into),
            series: None,
        }
    }
    #[test]
    fn contact_references_name_a_normalized_address_and_one_target() {
        for valid in [
            "pete@example.com",
            "pete.b+ops@example.co.uk",
            "δοκιμή@example.com",
        ] {
            let reference = contact(valid, Some("c1"));
            assert_eq!(
                NoteReference::parse(&reference.href().unwrap()),
                Some(reference)
            );
        }
        for invalid in [
            "Pete@example.com",
            " pete@example.com",
            "pete example@example.com",
            "pete",
            "@example.com",
            "pete@",
            "a@b@example.com",
        ] {
            assert!(!contact(invalid, None).validate(), "{invalid}");
        }
        assert_eq!(
            normalize_contact_address("  Pete@Example.COM "),
            "pete@example.com"
        );
        // The contact id is a hint: with or without it, one target.
        assert_eq!(
            contact("pete@example.com", Some("c1")).target().unwrap(),
            contact("pete@example.com", None).target().unwrap()
        );
        assert_ne!(
            contact("pete@example.com", None).target().unwrap(),
            contact("frank@example.com", None).target().unwrap()
        );
        // Only a contact reference carries a contact id, and it carries
        // nothing else.
        let mut task = reference();
        task.contact = Some("c1".into());
        assert!(!task.validate());
        for extra in [
            serde_json::json!({"version":1,"kind":"contact","id":"pete@example.com","account":"you@example.com"}),
            serde_json::json!({"version":1,"kind":"contact","id":"pete@example.com","note":"n"}),
            serde_json::json!({"version":1,"kind":"contact","id":"pete@example.com","contact":null}),
            serde_json::json!({"version":1,"kind":"contact","id":"pete@example.com","contact":""}),
            serde_json::json!({"version":1,"kind":"contact","id":"pete@example.com","name":"Pete"}),
        ] {
            let href = format!(
                "threestrands:contact/{}",
                percent_encoding::utf8_percent_encode(
                    &extra.to_string(),
                    percent_encoding::NON_ALPHANUMERIC
                )
            );
            assert!(NoteReference::parse(&href).is_none(), "{extra}");
        }
        let long = format!("{}@example.com", "a".repeat(MAX_NOTE_REFERENCE_BYTES));
        assert!(contact(&long, None).href().is_err());
    }
    #[test]
    fn an_event_occurrence_names_itself_and_counts_toward_its_series() {
        let occurrence = NoteReference {
            version: 1,
            kind: "event".into(),
            id: "standup_1112".into(),
            account: Some("you@example.com".into()),
            calendar: Some("work".into()),
            note: None,
            contact: None,
            series: Some("standup".into()),
        };
        let bare = NoteReference {
            series: None,
            ..occurrence.clone()
        };
        let whole = NoteReference {
            id: "standup".into(),
            ..bare.clone()
        };
        assert_eq!(
            NoteReference::parse(&occurrence.href().unwrap()),
            Some(occurrence.clone())
        );
        assert_eq!(occurrence.target().unwrap(), bare.target().unwrap());
        assert_eq!(
            occurrence.series_target().unwrap(),
            Some(whole.target().unwrap())
        );
        assert_eq!(bare.series_target().unwrap(), None);
        // A series names only an event occurrence, never itself.
        assert!(!NoteReference {
            series: Some("standup_1112".into()),
            ..occurrence.clone()
        }
        .validate());
        let mut task = reference();
        task.series = Some("s".into());
        assert!(!task.validate());
        let json = serde_json::json!({"version":1,"kind":"event","id":"e","account":"a","calendar":"c","series":null});
        assert!(NoteReference::parse(&format!(
            "threestrands:event/{}",
            percent_encoding::utf8_percent_encode(
                &json.to_string(),
                percent_encoding::NON_ALPHANUMERIC
            )
        ))
        .is_none());
    }
    #[test]
    fn wiki_links_and_nested_brackets_do_not_capture_a_later_entity_links_label() {
        let href = reference().href().unwrap();
        let body = format!("See [[Plan]] and [Task]({href}); [nested [label]]({href})");
        let parsed = parse("n", &body).unwrap();
        assert_eq!(
            parsed
                .links
                .iter()
                .map(|link| link.label.as_str())
                .collect::<Vec<_>>(),
            ["Task", "nested [label]"]
        );
    }

    #[test]
    fn unmatched_punctuation_before_a_reference_is_bounded_work() {
        let href = reference().href().unwrap();
        let body = format!("{} [Task]({href})", "[a".repeat(10_000));
        assert_eq!(parse("n", &body).unwrap().links[0].label, "Task");
        assert_eq!(
            crate::notes::plain_text(&"<x".repeat(10_000)),
            "<x".repeat(10_000)
        );
    }

    #[test]
    fn task_and_reference_count_boundaries_never_silently_truncate() {
        for count in [MAX_NOTE_TASKS - 1, MAX_NOTE_TASKS, MAX_NOTE_TASKS + 1] {
            assert_eq!(
                parse("n", &"- ( ) task\n".repeat(count)).is_ok(),
                count <= MAX_NOTE_TASKS
            );
        }
        let link = format!("[x]({})\n", reference().href().unwrap());
        for count in [
            MAX_NOTE_ENTITY_LINKS - 1,
            MAX_NOTE_ENTITY_LINKS,
            MAX_NOTE_ENTITY_LINKS + 1,
        ] {
            assert_eq!(
                parse("n", &link.repeat(count)).is_ok(),
                count <= MAX_NOTE_ENTITY_LINKS
            );
        }
        for count in [
            MAX_NOTE_REFERENCE_BYTES - 1,
            MAX_NOTE_REFERENCE_BYTES,
            MAX_NOTE_REFERENCE_BYTES + 1,
        ] {
            let mut r = reference();
            r.id = "a".into();
            let base = r.href().unwrap().len() - 1;
            r.id = "a".repeat(count - base);
            assert_eq!(r.href().is_ok(), count <= MAX_NOTE_REFERENCE_BYTES);
        }
        assert!(!parse("n", "```\ncode\n~~~\n```\n").unwrap().unclosed_fence);
        assert!(parse("n", "~~~\ncode\n").unwrap().unclosed_fence);
    }
}
