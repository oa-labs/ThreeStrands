//! Import a folder of Markdown notes: a Reflect folder, or one Export to
//! Markdown Folder wrote.
//!
//! Importing has two steps. [`plan`] reads the folder and decides
//! everything: which files become notes, each note's id, title, dates, and
//! text, and which pictures and files it attaches. The user sees that
//! summary, with what can't be imported and why, before [`run`] writes
//! anything.
//!
//! - **Daily notes** are `YYYY-MM-DD.md` files in a folder named `daily`.
//!   Every other `.md` file is a regular note, whatever folder it is in.
//! - **Titles** come from `title:` in the front matter, else a leading
//!   `# Heading`, which is then removed from the text, else the file name.
//!   Characters a title can't hold are replaced, and a title already in use
//!   gets " 2", " 3", and so on.
//! - **Ids**: a daily note's id is its date. A front matter `id` that is a
//!   ThreeStrands note id is kept; any other `id` (Reflect's own), or the
//!   file's path when there is none, is hashed into a stable id. Importing
//!   the same folder again skips every note already here; nothing is ever
//!   replaced.
//! - **Links** to a note's file name, an alias, or a relative path to the
//!   note's file (`[text](../notes/Plan.md)`) become `[[Title]]` links.
//! - **Files**: a link or image whose destination is a file inside the
//!   folder becomes an attachment of the note, under an id derived from the
//!   note and the file, so two devices importing one folder agree. Nothing outside the folder
//!   is ever read, and symbolic links are never followed.
//! - **Front matter** keys Reflect keeps for itself (`id`, `summary`, and
//!   the like) are dropped; anything else stays at the top of the note.
//! - **Not imported**, each reported with its reason: a note too long to
//!   sync, a file that isn't UTF-8 text, Reflect's copies of conflicting
//!   edits (`name.md~other device`), and a second copy of a note (the same
//!   `id`) with the same text.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use threestrands_sync_protocol::{
    note_body_bytes, MAX_NOTE_ATTACHMENT_BYTES, MAX_NOTE_BODY_BYTES, MAX_NOTE_TITLE_CHARS,
};

use crate::db::note_attachments::ATTACHMENT_SCHEME;
use crate::models::Note;
use crate::notes::{clean_note_title, daily_note_id, is_daily_date, title_key};

/// Most entries read from the folder, so choosing a huge folder by mistake
/// stops quickly.
pub(crate) const MAX_IMPORT_ENTRIES: usize = 20_000;
/// Most notes one import takes.
pub(crate) const MAX_IMPORT_NOTES: usize = 5_000;
/// Deepest folder level read.
const MAX_DEPTH: usize = 12;
/// A Markdown file larger than this can't be a note that syncs, so it isn't
/// read.
const MAX_NOTE_FILE_BYTES: u64 = 4 * MAX_NOTE_BODY_BYTES as u64;
/// Front matter keys Reflect and ThreeStrands keep for themselves.
const DROPPED_KEYS: [&str; 8] = [
    "id",
    "title",
    "created",
    "updated",
    "summary",
    "snippet",
    "summary_source_hash",
    "pinned",
];

/// Something in the folder that won't be imported, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportProblem {
    /// The file's path inside the folder.
    pub file: String,
    pub reason: String,
}

/// A file a note will attach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedFile {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) declared_type: String,
    /// Its path inside the folder.
    pub(crate) relative: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlannedNote {
    pub(crate) note: Note,
    pub(crate) files: Vec<PlannedFile>,
    pub(crate) source: String,
}

/// Everything an import will do, decided before anything is written.
#[derive(Debug, Clone)]
pub(crate) struct ImportPlan {
    pub(crate) folder: PathBuf,
    pub(crate) notes: Vec<PlannedNote>,
    pub(crate) already_here: usize,
    pub(crate) problems: Vec<ImportProblem>,
}

/// What the user sees before importing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub path: String,
    pub notes: usize,
    pub daily_notes: usize,
    pub files: usize,
    /// Notes this device already has, which are skipped.
    pub already_here: usize,
    pub problems: Vec<ImportProblem>,
}

/// What an import did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    pub notes: usize,
    pub files: usize,
    /// Notes that couldn't be saved after all, such as when notes ran out
    /// of sync space.
    pub problems: Vec<ImportProblem>,
}

impl ImportPlan {
    pub(crate) fn summary(&self) -> ImportSummary {
        let daily = self
            .notes
            .iter()
            .filter(|planned| planned.note.daily_date.is_some())
            .count();
        ImportSummary {
            path: self.folder.to_string_lossy().into_owned(),
            notes: self.notes.len() - daily,
            daily_notes: daily,
            files: self.notes.iter().map(|planned| planned.files.len()).sum(),
            already_here: self.already_here,
            problems: self.problems.clone(),
        }
    }
}

/// The scanned import waiting for the user to confirm it, kept here so the
/// page never names a folder.
#[derive(Default)]
pub struct PendingImport(Mutex<Option<ImportPlan>>);

impl PendingImport {
    pub(crate) fn set(&self, plan: Option<ImportPlan>) {
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = plan;
    }

    pub(crate) fn take(&self) -> Option<ImportPlan> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

/// What this device already has: every note id, and the title keys of
/// notes outside Trash.
pub(crate) struct Existing {
    pub(crate) ids: HashSet<String>,
    pub(crate) titles: HashSet<String>,
}

// ---- Reading the folder ----

/// Every file under `folder` worth looking at, as paths inside it with `/`
/// separators: hidden entries (`.git`, `.reflect`) and symbolic links are
/// skipped.
fn walk(folder: &Path) -> Result<Vec<String>, String> {
    let mut found = Vec::new();
    let mut pending = vec![(PathBuf::new(), 0usize)];
    let mut seen = 0usize;
    while let Some((relative, depth)) = pending.pop() {
        let entries = fs::read_dir(folder.join(&relative))
            .map_err(|error| format!("Could not read the folder: {error}"))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("Could not read the folder: {error}"))?;
            seen += 1;
            if seen > MAX_IMPORT_ENTRIES {
                return Err(format!(
                    "This folder holds more than {MAX_IMPORT_ENTRIES} files. Choose the folder that holds your notes."
                ));
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = relative.join(&name);
            if kind.is_dir() && depth < MAX_DEPTH {
                pending.push((path, depth + 1));
            } else if kind.is_file() {
                found.push(slash_path(&path));
            }
        }
    }
    found.sort();
    Ok(found)
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn modified(path: &Path) -> String {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map(|time| DateTime::<Utc>::from(time).to_rfc3339())
        .unwrap_or_else(|_| Utc::now().to_rfc3339())
}

// ---- Front matter ----

/// A note file split into its front matter, as `(key, value, lines)` in
/// order, and the text after it.
struct Parsed {
    keys: Vec<(String, Option<String>, Vec<String>)>,
    body: String,
}

fn scalar(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.starts_with('"') {
        return serde_json::from_str::<String>(raw).ok();
    }
    Some(
        raw.strip_prefix('\'')
            .and_then(|inner| inner.strip_suffix('\''))
            .unwrap_or(raw)
            .to_string(),
    )
}

fn parse(text: &str) -> Parsed {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return Parsed {
            keys: Vec::new(),
            body: String::new(),
        };
    };
    if first.trim_end() != "---" {
        return Parsed {
            keys: Vec::new(),
            body: text.to_string(),
        };
    }
    let mut keys: Vec<(String, Option<String>, Vec<String>)> = Vec::new();
    let mut consumed = first.len();
    let mut closed = false;
    for line in lines {
        consumed += line.len();
        let bare = line.trim_end();
        if bare == "---" || bare == "..." {
            closed = true;
            break;
        }
        let key_end = bare.find(':').filter(|end| {
            *end > 0
                && bare[..*end]
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        });
        match key_end {
            Some(end) => keys.push((
                bare[..end].to_string(),
                scalar(&bare[end + 1..]),
                vec![bare.to_string()],
            )),
            None => {
                if let Some(last) = keys.last_mut() {
                    last.2.push(bare.to_string());
                }
            }
        }
    }
    if !closed {
        return Parsed {
            keys: Vec::new(),
            body: text.to_string(),
        };
    }
    Parsed {
        keys,
        body: text[consumed..]
            .trim_start_matches(['\r', '\n'])
            .to_string(),
    }
}

fn value<'a>(parsed: &'a Parsed, key: &str) -> Option<&'a str> {
    parsed
        .keys
        .iter()
        .find(|(name, _, _)| name == key)
        .and_then(|(_, value, _)| value.as_deref())
}

fn list(parsed: &Parsed, key: &str) -> Vec<String> {
    parsed
        .keys
        .iter()
        .filter(|(name, _, _)| name == key)
        .flat_map(|(_, _, lines)| lines.iter().skip(1))
        .filter_map(|line| line.trim().strip_prefix("- ").and_then(scalar))
        .collect()
}

fn is_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.hyphenated().to_string() == value)
}

fn hashed_id(source: &str) -> String {
    let digest = Sha256::digest(source.as_bytes());
    format!(
        "import-{}",
        digest[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

/// A title a regular note can hold, from whatever the file had.
fn usable_title(raw: &str) -> String {
    let replaced: String = raw
        .chars()
        .map(|ch| match ch {
            '[' | ']' | '|' => '-',
            ch if ch.is_control() => ' ',
            ch => ch,
        })
        .collect();
    let mut title: String = replaced
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_NOTE_TITLE_CHARS)
        .collect();
    if title.is_empty() {
        title = "Untitled".to_string();
    }
    if clean_note_title(&title).is_err() {
        // A date names a daily note.
        title = format!("{title} (note)");
    }
    title
}

/// `title`, or `title 2`, `title 3`, … if it is taken; takes it.
fn free_title(title: &str, taken: &mut HashSet<String>) -> String {
    let mut attempt = 1;
    loop {
        let candidate = if attempt == 1 {
            title.to_string()
        } else {
            let suffix = format!(" {attempt}");
            let room = MAX_NOTE_TITLE_CHARS - suffix.chars().count();
            format!("{}{suffix}", title.chars().take(room).collect::<String>())
        };
        if taken.insert(title_key(&candidate)) {
            return candidate;
        }
        attempt += 1;
    }
}

// ---- Links ----

/// A link found in a line of Markdown.
enum Link<'a> {
    /// `[[inner]]`
    Wiki(&'a str),
    /// `[label](destination rest)` or, with `image`, `![label](…)`.
    Inline {
        image: bool,
        label: &'a str,
        destination: &'a str,
        rest: &'a str,
    },
}

/// `line` with each link `replace` returns text for replaced by that text.
/// Escaped characters and inline code are left alone.
fn rewrite_line(line: &str, replace: &mut impl FnMut(Link<'_>) -> Option<String>) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let (mut index, mut copied) = (0usize, 0usize);
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => {
                index += 1;
                index += line[index..].chars().next().map_or(0, char::len_utf8);
            }
            b'`' => {
                let run = bytes[index..]
                    .iter()
                    .take_while(|byte| **byte == b'`')
                    .count();
                let fence = "`".repeat(run);
                index = line[index + run..]
                    .find(&fence)
                    .map_or(bytes.len(), |end| index + run + end + run);
            }
            b'[' if bytes.get(index + 1) == Some(&b'[') => {
                let start = index + 2;
                match line[start..].find("]]") {
                    Some(length) if !line[start..start + length].contains('[') => {
                        let end = start + length + 2;
                        if let Some(text) = replace(Link::Wiki(&line[start..start + length])) {
                            out.push_str(&line[copied..index]);
                            out.push_str(&text);
                            copied = end;
                        }
                        index = end;
                    }
                    _ => index += 1,
                }
            }
            b'!' | b'[' => {
                let image = bytes[index] == b'!';
                let open = if image { index + 1 } else { index };
                if bytes.get(open) != Some(&b'[') {
                    index += 1;
                    continue;
                }
                let found = line[open + 1..].find(']').and_then(|length| {
                    let close = open + 1 + length;
                    let label = &line[open + 1..close];
                    (!label.contains('[') && bytes.get(close + 1) == Some(&b'('))
                        .then_some(())
                        .and_then(|()| line[close + 2..].find(')'))
                        .map(|length| (label, close + 2, close + 2 + length))
                });
                let Some((label, inside_start, inside_end)) = found else {
                    index += 1;
                    continue;
                };
                let inside = &line[inside_start..inside_end];
                let (destination, rest) = match inside.strip_prefix('<') {
                    Some(after) => match after.find('>') {
                        Some(end) => (&after[..end], &after[end + 1..]),
                        None => (inside, ""),
                    },
                    None => {
                        let end = inside.find(char::is_whitespace).unwrap_or(inside.len());
                        (&inside[..end], &inside[end..])
                    }
                };
                if let Some(text) = replace(Link::Inline {
                    image,
                    label,
                    destination,
                    rest,
                }) {
                    out.push_str(&line[copied..index]);
                    out.push_str(&text);
                    copied = inside_end + 1;
                }
                index = inside_end + 1;
            }
            _ => index += 1,
        }
    }
    out.push_str(&line[copied..]);
    out
}

/// `body` with `rewrite_line` applied outside fenced code blocks.
fn rewrite_body(body: &str, replace: &mut impl FnMut(Link<'_>) -> Option<String>) -> String {
    let mut out = String::with_capacity(body.len());
    let mut fence: Option<String> = None;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let marker: String = trimmed
            .chars()
            .take_while(|ch| *ch == '`' || *ch == '~')
            .collect();
        let is_fence = marker.len() >= 3
            && (marker.chars().all(|ch| ch == '`') || marker.chars().all(|ch| ch == '~'));
        match &fence {
            Some(open) => {
                if is_fence && marker.starts_with(open.as_str()) {
                    fence = None;
                }
                out.push_str(line);
            }
            None if is_fence => {
                fence = Some(marker);
                out.push_str(line);
            }
            None => out.push_str(&rewrite_line(line, replace)),
        }
    }
    out
}

/// A link destination's path inside the folder, if it names a regular file
/// there: relative to the note's own folder first, then to the folder's
/// root, where Reflect writes `assets/…`. Web addresses, anchors, and paths
/// that leave the folder name nothing.
fn resolve(folder: &Path, note_dir: &str, destination: &str) -> Option<String> {
    let decoded = percent_encoding::percent_decode_str(destination)
        .decode_utf8()
        .ok()?;
    let decoded = decoded.split('#').next().unwrap_or_default();
    if decoded.is_empty() || decoded.contains(':') || decoded.starts_with('/') {
        return None;
    }
    let bases: &[&str] = if note_dir.is_empty() {
        &[""]
    } else {
        &[note_dir, ""]
    };
    for base in bases {
        let mut parts: Vec<String> = Vec::new();
        let mut inside = true;
        for component in Path::new(base).join(decoded).components() {
            match component {
                Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
                Component::CurDir => {}
                Component::ParentDir => {
                    if parts.pop().is_none() {
                        inside = false;
                        break;
                    }
                }
                _ => {
                    inside = false;
                    break;
                }
            }
        }
        if !inside || parts.is_empty() || parts.iter().any(|part| part.starts_with('.')) {
            continue;
        }
        let relative = parts.join("/");
        // No part of the path may be a link, so nothing outside is reached.
        let mut path = folder.to_path_buf();
        let plain = parts.iter().all(|part| {
            path.push(part);
            fs::symlink_metadata(&path).is_ok_and(|metadata| !metadata.file_type().is_symlink())
        });
        if plain && fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
            return Some(relative);
        }
    }
    None
}

/// The type a file's extension suggests. Images are identified from their
/// bytes when stored, whatever this says.
fn declared_type(name: &str) -> &'static str {
    let extension = name.rsplit('.').next().unwrap_or_default().to_lowercase();
    match extension.as_str() {
        "pdf" => "application/pdf",
        "txt" | "md" => "text/plain",
        "csv" => "text/csv",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "zip" => "application/zip",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        _ => "application/octet-stream",
    }
}

/// A file's attachment id, derived from its note, its path, and its size,
/// so importing the same folder on two devices writes the same note text
/// and the same attachments rather than two versions of each.
fn file_id(note_id: &str, relative: &str, size: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"threestrands/notes-import/attachment-id/v1\0");
    hasher.update(note_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(relative.as_bytes());
    hasher.update(b"\0");
    hasher.update(size.to_be_bytes());
    hasher.finalize()[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The text of `[[title]]`, shown as `label` when that differs.
fn wiki(title: &str, label: &str) -> String {
    let label = label.trim();
    if label.is_empty()
        || title_key(label) == title_key(title)
        || label.contains(['[', ']', '|', '\n'])
    {
        format!("[[{title}]]")
    } else {
        format!("[[{title}|{label}]]")
    }
}

// ---- Planning ----

/// One note file after its first reading.
struct Draft {
    source: String,
    note: Note,
    /// Names that should link to this note: its file name, then its
    /// aliases (`true`), which stay as a link's shown text.
    names: Vec<(String, bool)>,
}

/// Reads `folder` and decides what an import would do.
pub(crate) fn plan(folder: &Path, existing: &Existing) -> Result<ImportPlan, String> {
    let files = walk(folder)?;
    let mut problems = Vec::new();
    let mut drafts: Vec<Draft> = Vec::new();
    let mut sources: HashMap<String, usize> = HashMap::new();
    let mut taken = existing.titles.clone();
    let mut already_here = 0;

    for relative in &files {
        let lower = relative.to_lowercase();
        if lower.ends_with(".reflect.md") {
            // Reflect's description of a picture or PDF, not a note.
            continue;
        }
        if lower.contains(".md~") {
            problems.push(ImportProblem {
                file: relative.clone(),
                reason: "A copy Reflect kept of conflicting edits. Compare it with the note and copy in anything you need.".into(),
            });
            continue;
        }
        if !lower.ends_with(".md") {
            continue;
        }
        if drafts.len() == MAX_IMPORT_NOTES {
            return Err(format!(
                "This folder holds more than {MAX_IMPORT_NOTES} notes. Import them in smaller folders."
            ));
        }
        let path = folder.join(relative);
        if fs::metadata(&path).is_ok_and(|metadata| metadata.len() > MAX_NOTE_FILE_BYTES) {
            problems.push(ImportProblem {
                file: relative.clone(),
                reason: "Too long to sync. A note can hold about 96 KB.".into(),
            });
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            problems.push(ImportProblem {
                file: relative.clone(),
                reason: "Not readable as text.".into(),
            });
            continue;
        };
        let parsed = parse(&text);
        let (dir, file_name) = relative.rsplit_once('/').unwrap_or(("", relative.as_str()));
        let stem = &file_name[..file_name.len() - 3];
        let parent = dir.rsplit('/').next().unwrap_or_default();
        let daily =
            (parent.eq_ignore_ascii_case("daily") && is_daily_date(stem)).then(|| stem.to_string());

        let mut body = parsed.body.clone();
        let explicit_title = value(&parsed, "title").filter(|title| !title.trim().is_empty());
        let title = match (&daily, explicit_title) {
            (Some(date), _) => date.clone(),
            (None, Some(title)) => {
                // An explicit title keeps a leading body heading as content.
                usable_title(title)
            }
            (None, None) => {
                let heading = body
                    .lines()
                    .next()
                    .and_then(|line| line.strip_prefix("# "))
                    .map(str::trim)
                    .filter(|heading| !heading.is_empty())
                    .map(str::to_string);
                if heading.is_some() {
                    body = body
                        .split_once('\n')
                        .map_or(String::new(), |(_, rest)| rest.to_string())
                        .trim_start_matches(['\r', '\n'])
                        .to_string();
                }
                usable_title(&heading.unwrap_or_else(|| stem.to_string()))
            }
        };
        let kept: Vec<&String> = parsed
            .keys
            .iter()
            .filter(|(key, _, _)| !DROPPED_KEYS.contains(&key.as_str()))
            .flat_map(|(_, _, lines)| lines)
            .collect();
        if !kept.is_empty() {
            let block: Vec<&str> = kept.iter().map(|line| line.as_str()).collect();
            body = format!("---\n{}\n---\n\n{body}", block.join("\n"));
        }

        let source_id = value(&parsed, "id").map(str::to_string);
        let id = match (&daily, &source_id) {
            (Some(date), _) => daily_note_id(date),
            (None, Some(id)) if is_uuid(id) => id.clone(),
            (None, Some(id)) => hashed_id(id),
            (None, None) => hashed_id(relative),
        };
        let mut id = id;
        if let Some(&first) = sources.get(&id) {
            if drafts[first].note.body == body || daily.is_some() {
                problems.push(ImportProblem {
                    file: relative.clone(),
                    reason: format!(
                        "Another copy of {}, so it's imported once.",
                        drafts[first].source
                    ),
                });
                continue;
            }
            // Two different notes claiming one id (a sync copy edited
            // since): both are imported.
            id = hashed_id(relative);
        }
        if existing.ids.contains(&id) {
            already_here += 1;
            continue;
        }
        let created = value(&parsed, "created")
            .filter(|value| DateTime::parse_from_rfc3339(value).is_ok())
            .map(str::to_string)
            .unwrap_or_else(|| modified(&path));
        let updated = value(&parsed, "updated")
            .filter(|value| DateTime::parse_from_rfc3339(value).is_ok())
            .map(str::to_string)
            .unwrap_or_else(|| modified(&path));
        let title = if daily.is_some() {
            title
        } else {
            free_title(&title, &mut taken)
        };
        let mut names = vec![(stem.to_string(), false)];
        names.extend(
            list(&parsed, "aliases")
                .into_iter()
                .map(|alias| (alias, true)),
        );
        sources.insert(id.clone(), drafts.len());
        drafts.push(Draft {
            source: relative.clone(),
            names,
            note: Note {
                id,
                title,
                body,
                daily_date: daily,
                created_at: created,
                updated_at: updated,
                trashed: false,
                trashed_at: None,
                authored_title: None,
            },
        });
    }

    // Names other notes may use for each note: titles first, then file
    // names and aliases where no title claims them.
    let mut by_title: HashMap<String, String> = HashMap::new();
    let mut by_path: HashMap<String, String> = HashMap::new();
    // A note file's name, when only one note file has it: a link whose path
    // went stale (the file moved to another folder) still finds its note.
    let mut by_file_name: HashMap<String, Option<String>> = HashMap::new();
    for draft in &drafts {
        by_title
            .entry(title_key(&draft.note.title))
            .or_insert_with(|| draft.note.title.clone());
        by_path.insert(draft.source.to_lowercase(), draft.note.title.clone());
        let file_name = draft
            .source
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_lowercase();
        by_file_name
            .entry(file_name)
            .and_modify(|title| *title = None)
            .or_insert_with(|| Some(draft.note.title.clone()));
    }
    let mut by_name: HashMap<String, (String, bool)> = HashMap::new();
    for draft in &drafts {
        for (name, alias) in &draft.names {
            if !by_title.contains_key(&title_key(name)) {
                by_name
                    .entry(title_key(name))
                    .or_insert_with(|| (draft.note.title.clone(), *alias));
            }
        }
    }

    let mut notes = Vec::with_capacity(drafts.len());
    for draft in drafts {
        let note_dir = draft
            .source
            .rsplit_once('/')
            .map_or("", |(dir, _)| dir)
            .to_string();
        let mut files: Vec<PlannedFile> = Vec::new();
        let mut file_problems = Vec::new();
        let body = rewrite_body(&draft.note.body, &mut |link| match link {
            Link::Wiki(inner) => {
                let (target, label) = inner.split_once('|').unwrap_or((inner, ""));
                let key = title_key(target);
                if by_title.contains_key(&key) || is_daily_date(target.trim()) {
                    return None;
                }
                let (title, alias) = by_name.get(&key)?;
                // An alias reads as the writer chose; a file name reads as the title.
                let shown = if !label.is_empty() {
                    label
                } else if *alias {
                    target
                } else {
                    ""
                };
                Some(wiki(title, shown))
            }
            Link::Inline {
                image,
                label,
                destination,
                rest,
            } => {
                let Some(relative) = resolve(folder, &note_dir, destination) else {
                    let decoded = percent_encoding::percent_decode_str(destination)
                        .decode_utf8()
                        .ok()?;
                    if image || decoded.contains(':') || !decoded.to_lowercase().ends_with(".md") {
                        return None;
                    }
                    let file_name = decoded.rsplit('/').next()?.to_lowercase();
                    let title = by_file_name.get(&file_name)?.as_ref()?;
                    return Some(wiki(title, label));
                };
                if relative.to_lowercase().ends_with(".md") {
                    if image {
                        return None;
                    }
                    let title = by_path.get(&relative.to_lowercase())?;
                    return Some(wiki(title, label));
                }
                let size = fs::metadata(folder.join(&relative)).ok()?.len();
                if size > MAX_NOTE_ATTACHMENT_BYTES {
                    file_problems.push(ImportProblem {
                        file: relative,
                        reason: format!(
                            "Larger than 20 MB, the most a note can hold, so {} keeps its link to it.",
                            draft.source
                        ),
                    });
                    return None;
                }
                let id = match files.iter().find(|file| file.relative == relative) {
                    Some(file) => file.id.clone(),
                    None => {
                        let name = relative.rsplit('/').next().unwrap_or_default().to_string();
                        let id = file_id(&draft.note.id, &relative, size);
                        files.push(PlannedFile {
                            id: id.clone(),
                            declared_type: declared_type(&name).to_string(),
                            name,
                            relative,
                        });
                        id
                    }
                };
                let bang = if image { "!" } else { "" };
                Some(format!("{bang}[{label}]({ATTACHMENT_SCHEME}{id}{rest})"))
            }
        });
        if note_body_bytes(&body) > MAX_NOTE_BODY_BYTES {
            problems.push(ImportProblem {
                file: draft.source,
                reason: "Too long to sync. A note can hold about 96 KB.".into(),
            });
            continue;
        }
        problems.extend(file_problems);
        notes.push(PlannedNote {
            note: Note { body, ..draft.note },
            files,
            source: draft.source,
        });
    }
    Ok(ImportPlan {
        folder: folder.to_path_buf(),
        notes,
        already_here,
        problems,
    })
}

/// Reads a planned file's bytes, checking again that it is still a plain
/// file inside the folder and small enough.
fn read_file(folder: &Path, file: &PlannedFile) -> Result<Vec<u8>, String> {
    let mut path = folder.to_path_buf();
    for part in file.relative.split('/') {
        path.push(part);
        if fs::symlink_metadata(&path).map_or(true, |metadata| metadata.file_type().is_symlink()) {
            return Err(format!(
                "{} changed since the folder was read.",
                file.relative
            ));
        }
    }
    let bytes =
        fs::read(&path).map_err(|error| format!("Could not read {}: {error}", file.relative))?;
    if bytes.len() as u64 > MAX_NOTE_ATTACHMENT_BYTES {
        return Err(format!("{} is larger than 20 MB.", file.relative));
    }
    Ok(bytes)
}

/// Writes a plan's notes through `save`, which stores one note and its
/// files and returns false when the note is already here. A note that
/// can't be saved, or whose files can't be read, is reported and the rest
/// go on.
pub(crate) fn run(
    plan: ImportPlan,
    mut save: impl FnMut(Note, &[(String, String, String, Vec<u8>)]) -> Result<bool, String>,
) -> ImportOutcome {
    let mut outcome = ImportOutcome {
        notes: 0,
        files: 0,
        problems: Vec::new(),
    };
    for planned in plan.notes {
        let files: Result<Vec<_>, String> = planned
            .files
            .iter()
            .map(|file| {
                read_file(&plan.folder, file).map(|bytes| {
                    (
                        file.id.clone(),
                        file.name.clone(),
                        file.declared_type.clone(),
                        bytes,
                    )
                })
            })
            .collect();
        let result = files.and_then(|files| {
            let count = files.len();
            save(planned.note, &files).map(|saved| (saved, count))
        });
        match result {
            Ok((true, count)) => {
                outcome.notes += 1;
                outcome.files += count;
            }
            Ok((false, _)) => {}
            Err(reason) => outcome.problems.push(ImportProblem {
                file: planned.source,
                reason,
            }),
        }
    }
    outcome
}

#[cfg(test)]
#[path = "notes_import_tests.rs"]
mod tests;
