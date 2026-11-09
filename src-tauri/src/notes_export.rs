//! Export to Markdown Folder: every note outside Trash, written as Markdown
//! files in Reflect's layout so a later import can read them back.
//!
//! ```text
//! <folder>/daily/2026-11-09.md
//! <folder>/notes/<title>.md
//! ```
//!
//! The export writes only inside the chosen folder. It never writes through
//! a symbolic link, never replaces a file it didn't write, and only reuses a
//! folder that is empty or holds an earlier export (named by
//! [`MANIFEST_NAME`]); replacing an earlier export removes only the files
//! that export listed.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::models::Note;

/// The file an export leaves in its folder, listing what it wrote, so the
/// next export there can replace exactly those files.
pub(crate) const MANIFEST_NAME: &str = ".threestrands-notes-export.json";
const MANIFEST_VERSION: u32 = 1;
const DAILY_DIR: &str = "daily";
const NOTES_DIR: &str = "notes";
/// Most bytes of a title kept in its file name, leaving room for a clash
/// suffix and `.md` under the 255-byte limit common to file systems.
pub(crate) const MAX_FILE_STEM_BYTES: usize = 200;
/// Files the system leaves in folders, which don't make a folder "in use".
const SYSTEM_FILES: [&str; 2] = [".DS_Store", "Thumbs.db"];
/// Names Windows reserves for devices, whatever the extension.
const WINDOWS_RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// What the chosen folder holds, which decides whether exporting there
/// needs the user to confirm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FolderState {
    Empty,
    /// An earlier export, which exporting again replaces.
    EarlierExport,
    /// Other files: the export refuses it.
    InUse,
}

/// The folder the user chose, shown before exporting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportFolder {
    pub path: String,
    pub state: FolderState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOutcome {
    pub path: String,
    pub notes: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    version: u32,
    files: Vec<String>,
}

/// The folder chosen for an export, kept here rather than sent back by the
/// page, so an export only ever writes where the user pointed the picker.
#[derive(Default)]
pub struct ChosenFolder(Mutex<Option<PathBuf>>);

impl ChosenFolder {
    pub(crate) fn set(&self, path: Option<PathBuf>) {
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = path;
    }

    pub(crate) fn get(&self) -> Option<PathBuf> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

/// What `folder` holds.
pub(crate) fn folder_state(folder: &Path) -> Result<FolderState, String> {
    let entries =
        fs::read_dir(folder).map_err(|error| format!("Could not read the folder: {error}"))?;
    let mut in_use = false;
    for entry in entries {
        let entry = entry.map_err(|error| format!("Could not read the folder: {error}"))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == MANIFEST_NAME {
            return Ok(FolderState::EarlierExport);
        }
        if !SYSTEM_FILES.contains(&name.as_ref()) {
            in_use = true;
        }
    }
    Ok(if in_use {
        FolderState::InUse
    } else {
        FolderState::Empty
    })
}

/// A file name for a note's title that is safe on macOS, Windows, and
/// Linux: separators, reserved characters, and control characters become
/// `-`, a leading dot (a hidden file) and trailing dots or spaces (dropped
/// by Windows) are replaced, device names get a suffix, and the name is
/// cut to [`MAX_FILE_STEM_BYTES`] at a character boundary.
pub(crate) fn file_stem(title: &str) -> String {
    let mut stem: String = title
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                '-'
            } else {
                ch
            }
        })
        .collect();
    if stem.len() > MAX_FILE_STEM_BYTES {
        let mut end = MAX_FILE_STEM_BYTES;
        while !stem.is_char_boundary(end) {
            end -= 1;
        }
        stem.truncate(end);
    }
    let trimmed = stem.trim_end_matches(['.', ' ']).len();
    if trimmed < stem.len() {
        stem.truncate(trimmed);
        stem.push('-');
    }
    if stem.starts_with('.') {
        stem.replace_range(0..1, "-");
    }
    if stem.trim().is_empty() {
        stem = "Untitled".to_string();
    }
    let base = stem
        .split('.')
        .next()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    if WINDOWS_RESERVED.contains(&base.as_str()) {
        stem.push('-');
    }
    stem
}

/// The file's text: front matter naming the note, then its Markdown.
pub(crate) fn file_text(note: &Note) -> String {
    let quote = |value: &str| serde_json::to_string(value).unwrap_or_default();
    format!(
        "---\nid: {}\ncreated: {}\nupdated: {}\n---\n\n{}",
        quote(&note.id),
        quote(&note.created_at),
        quote(&note.updated_at),
        note.body
    )
}

/// A subfolder of the export, created if missing. One that is a symbolic
/// link, or not a folder, is refused, so nothing is written outside.
fn subfolder(folder: &Path, name: &str) -> Result<PathBuf, String> {
    let path = folder.join(name);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() => Ok(path),
        Ok(_) => Err(format!(
            "The folder's \u{201c}{name}\u{201d} is a link or a file, so the export won't write there."
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&path).map_err(|error| format!("Could not create \u{201c}{name}\u{201d}: {error}"))?;
            Ok(path)
        }
        Err(error) => Err(format!("Could not read \u{201c}{name}\u{201d}: {error}")),
    }
}

/// A listed path, if it stays inside the export: relative, with only
/// ordinary parts.
fn contained(folder: &Path, relative: &str) -> Option<PathBuf> {
    let path = Path::new(relative);
    path.components()
        .all(|part| matches!(part, Component::Normal(_)))
        .then(|| folder.join(path))
}

/// Removes the files an earlier export listed, when they are still
/// ordinary files inside the folder. Anything else stays.
fn remove_earlier_export(folder: &Path) -> Result<(), String> {
    let manifest_path = folder.join(MANIFEST_NAME);
    let metadata = fs::symlink_metadata(&manifest_path)
        .map_err(|error| format!("Could not read the earlier export: {error}"))?;
    if !metadata.is_file() {
        return Err(
            "The earlier export's list of files is a link, so it can't be replaced.".to_string(),
        );
    }
    let text = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("Could not read the earlier export: {error}"))?;
    let manifest: Manifest = serde_json::from_str(&text).map_err(|_| {
        "The earlier export's list of files is damaged, so it can't be replaced.".to_string()
    })?;
    for relative in &manifest.files {
        let Some(path) = contained(folder, relative) else {
            continue;
        };
        if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
            fs::remove_file(&path)
                .map_err(|error| format!("Could not remove {relative}: {error}"))?;
        }
    }
    fs::remove_file(&manifest_path)
        .map_err(|error| format!("Could not replace the earlier export: {error}"))
}

/// Writes `notes` to `folder`. An earlier export there is replaced only
/// when `replace` is set; a folder holding anything else is refused.
pub(crate) fn export(
    folder: &Path,
    notes: &[Note],
    replace: bool,
) -> Result<ExportOutcome, String> {
    match folder_state(folder)? {
        FolderState::Empty => {}
        FolderState::EarlierExport if replace => remove_earlier_export(folder)?,
        FolderState::EarlierExport => {
            return Err("This folder holds an earlier export. Confirm replacing it to export here.".to_string())
        }
        FolderState::InUse => {
            return Err(
                "This folder already holds other files. Choose an empty folder, or one an earlier export of notes wrote."
                    .to_string(),
            )
        }
    }
    let daily = subfolder(folder, DAILY_DIR)?;
    let regular = subfolder(folder, NOTES_DIR)?;

    // Names are chosen before anything is written, and the list of them is
    // written first, so an export that stops partway can still be replaced.
    let mut taken: HashSet<String> = HashSet::new();
    let mut planned: Vec<(PathBuf, String, &Note)> = Vec::with_capacity(notes.len());
    for note in notes {
        let (dir, dir_name, stem) = match &note.daily_date {
            Some(date) => (&daily, DAILY_DIR, file_stem(date)),
            None => (&regular, NOTES_DIR, file_stem(&note.title)),
        };
        let mut attempt = 1;
        let name = loop {
            let name = if attempt == 1 {
                format!("{stem}.md")
            } else {
                format!("{stem} {attempt}.md")
            };
            // Case is ignored, since macOS and Windows folders ignore it.
            let key = format!("{dir_name}/{}", name.to_lowercase());
            if !taken.contains(&key) && fs::symlink_metadata(dir.join(&name)).is_err() {
                taken.insert(key);
                break name;
            }
            attempt += 1;
        };
        planned.push((dir.join(&name), format!("{dir_name}/{name}"), note));
    }
    let manifest = Manifest {
        version: MANIFEST_VERSION,
        files: planned
            .iter()
            .map(|(_, relative, _)| relative.clone())
            .collect(),
    };
    write_new(
        &folder.join(MANIFEST_NAME),
        &serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?,
    )?;
    for (path, relative, note) in &planned {
        write_new(path, &file_text(note))
            .map_err(|error| format!("Could not write {relative}: {error}"))?;
    }
    Ok(ExportOutcome {
        path: folder.to_string_lossy().into_owned(),
        notes: planned.len(),
    })
}

/// Creates `path` and writes `text`, refusing a path that already exists,
/// including a symbolic link, so nothing is replaced or written through.
fn write_new(path: &Path, text: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.write_all(text.as_bytes())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder for one test, removed when it ends.
    struct TestFolder(PathBuf);

    impl TestFolder {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestFolder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn tempdir() -> TestFolder {
        let path = std::env::temp_dir().join(format!("notes-export-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        TestFolder(path)
    }

    fn note(id: &str, title: &str, daily: Option<&str>, body: &str) -> Note {
        Note {
            id: id.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            daily_date: daily.map(str::to_string),
            created_at: "2026-11-01T09:00:00Z".to_string(),
            updated_at: "2026-11-02T10:00:00Z".to_string(),
            trashed: false,
            trashed_at: None,
        }
    }

    fn tree(folder: &Path) -> Vec<String> {
        let mut files = Vec::new();
        for dir in ["", DAILY_DIR, NOTES_DIR] {
            let Ok(entries) = fs::read_dir(folder.join(dir)) else {
                continue;
            };
            for entry in entries.flatten() {
                if entry.file_type().unwrap().is_file() {
                    files.push(format!(
                        "{}{}",
                        if dir.is_empty() {
                            String::new()
                        } else {
                            format!("{dir}/")
                        },
                        entry.file_name().to_string_lossy()
                    ));
                }
            }
        }
        files.sort();
        files
    }

    #[test]
    fn file_names_are_safe_everywhere() {
        assert_eq!(file_stem("Q4 plan"), "Q4 plan");
        assert_eq!(file_stem("a/b\\c:d*e?f\"g<h>i|j"), "a-b-c-d-e-f-g-h-i-j");
        assert_eq!(file_stem(".hidden"), "-hidden");
        assert_eq!(file_stem("ends with dot."), "ends with dot-");
        assert_eq!(file_stem("ends with space "), "ends with space-");
        assert_eq!(file_stem("CON"), "CON-");
        assert_eq!(file_stem("com1.txt"), "com1.txt-");
        assert_eq!(file_stem("Console"), "Console");
        assert_eq!(file_stem("tab\there"), "tab-here");
        assert_eq!(file_stem("..."), "-");
    }

    #[test]
    fn file_names_are_cut_at_the_byte_limit_on_a_character_boundary() {
        let below = "a".repeat(MAX_FILE_STEM_BYTES - 1);
        assert_eq!(file_stem(&below), below);
        let at = "a".repeat(MAX_FILE_STEM_BYTES);
        assert_eq!(file_stem(&at), at);
        assert_eq!(file_stem(&format!("{at}b")), at);
        // A two-byte character straddling the limit is dropped whole.
        let straddling = format!("{}é", "a".repeat(MAX_FILE_STEM_BYTES - 1));
        assert_eq!(file_stem(&straddling), "a".repeat(MAX_FILE_STEM_BYTES - 1));
    }

    #[test]
    fn writes_daily_and_regular_notes_with_front_matter_and_numbered_clashes() {
        let folder = tempdir();
        let notes = [
            note(
                "daily:2026-11-09",
                "2026-11-09",
                Some("2026-11-09"),
                "Today [[Plan]]",
            ),
            note("n1", "Plan", None, "The plan"),
            note("n2", "a/b", None, ""),
            note("n3", "a:b", None, "second"),
            note("n4", "PLAN", None, "same name ignoring case"),
        ];
        let outcome = export(folder.path(), &notes, false).unwrap();
        assert_eq!(outcome.notes, 5);
        assert_eq!(
            tree(folder.path()),
            [
                MANIFEST_NAME,
                "daily/2026-11-09.md",
                "notes/PLAN 2.md",
                "notes/Plan.md",
                "notes/a-b 2.md",
                "notes/a-b.md"
            ]
        );
        assert_eq!(
            fs::read_to_string(folder.path().join("daily/2026-11-09.md")).unwrap(),
            "---\nid: \"daily:2026-11-09\"\ncreated: \"2026-11-01T09:00:00Z\"\nupdated: \"2026-11-02T10:00:00Z\"\n---\n\nToday [[Plan]]"
        );
    }

    #[test]
    fn refuses_a_folder_in_use_and_replaces_only_what_an_earlier_export_wrote() {
        let folder = tempdir();
        fs::write(folder.path().join(".DS_Store"), "").unwrap();
        assert_eq!(folder_state(folder.path()).unwrap(), FolderState::Empty);
        export(folder.path(), &[note("n1", "Old", None, "old")], false).unwrap();
        assert_eq!(
            folder_state(folder.path()).unwrap(),
            FolderState::EarlierExport
        );
        fs::write(folder.path().join("notes/mine.md"), "kept").unwrap();

        let error = export(folder.path(), &[note("n2", "New", None, "new")], false).unwrap_err();
        assert!(error.contains("Confirm replacing"), "{error}");
        export(
            folder.path(),
            &[note("n2", "New", None, "new"), note("n3", "mine", None, "")],
            true,
        )
        .unwrap();
        assert_eq!(
            tree(folder.path()),
            [
                ".DS_Store",
                MANIFEST_NAME,
                "notes/New.md",
                "notes/mine 2.md",
                "notes/mine.md"
            ]
        );
        assert_eq!(
            fs::read_to_string(folder.path().join("notes/mine.md")).unwrap(),
            "kept"
        );

        let other = tempdir();
        fs::write(other.path().join("report.pdf"), "").unwrap();
        assert_eq!(folder_state(other.path()).unwrap(), FolderState::InUse);
        assert!(export(other.path(), &[note("n1", "A", None, "")], true).is_err());
        assert_eq!(tree(other.path()), ["report.pdf"]);
    }

    #[test]
    fn a_damaged_or_escaping_list_never_removes_files_outside() {
        let outside = tempdir();
        fs::write(outside.path().join("precious.md"), "keep").unwrap();
        let folder = tempdir();
        let escape = outside.path().join("precious.md");
        let manifest = serde_json::json!({ "version": 1, "files": ["../precious.md", escape.to_string_lossy(), "notes/../../precious.md"] });
        fs::write(folder.path().join(MANIFEST_NAME), manifest.to_string()).unwrap();
        export(folder.path(), &[note("n1", "A", None, "")], true).unwrap();
        assert_eq!(
            fs::read_to_string(outside.path().join("precious.md")).unwrap(),
            "keep"
        );

        fs::write(folder.path().join(MANIFEST_NAME), "not json").unwrap();
        assert!(export(folder.path(), &[], true)
            .unwrap_err()
            .contains("damaged"));
    }

    #[cfg(unix)]
    #[test]
    fn never_writes_through_symbolic_links() {
        let outside = tempdir();
        let folder = tempdir();
        std::os::unix::fs::symlink(outside.path(), folder.path().join(NOTES_DIR)).unwrap();
        fs::write(
            folder.path().join(MANIFEST_NAME),
            r#"{"version":1,"files":[]}"#,
        )
        .unwrap();
        let error = export(folder.path(), &[note("n1", "A", None, "")], true).unwrap_err();
        assert!(error.contains("is a link"), "{error}");
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);

        // A link where a note's file would go is skipped for a numbered name, and its target untouched.
        let folder = tempdir();
        fs::create_dir(folder.path().join(NOTES_DIR)).unwrap();
        fs::write(outside.path().join("target.md"), "outside").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("target.md"),
            folder.path().join("notes/A.md"),
        )
        .unwrap();
        fs::write(
            folder.path().join(MANIFEST_NAME),
            r#"{"version":1,"files":["notes/A.md"]}"#,
        )
        .unwrap();
        export(folder.path(), &[note("n1", "A", None, "mine")], true).unwrap();
        assert_eq!(
            fs::read_to_string(outside.path().join("target.md")).unwrap(),
            "outside"
        );
        assert_eq!(
            fs::read_to_string(folder.path().join("notes/A 2.md"))
                .unwrap()
                .ends_with("mine"),
            true
        );
    }
}
