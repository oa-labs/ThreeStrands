//! Shows `.ics` files that macOS opens with ThreeStrands (from Finder, a
//! browser download, or another mail app's attachment) as calendar
//! invitations. Each file is read and parsed as soon as it arrives, since
//! downloads and attachment temp files may not last, then queued for the
//! webview like mail links are (see mail_links.rs).

use std::path::Path;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::calendar::{self, CalendarPreview, MAX_CALENDAR_BYTES};
use crate::limits::MAX_PENDING_CALENDAR_FILES;

/// Must match `CALENDAR_FILE_EVENT` in `src/calendarFiles.ts`.
const CALENDAR_FILE_EVENT: &str = "calendar-file-received";

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedCalendarFile {
    pub name: String,
    /// The parsed invitation, or None when `error` says why it could not be read.
    pub preview: Option<CalendarPreview>,
    pub error: Option<String>,
}

/// Files read but not yet taken by the webview.
#[derive(Default)]
pub struct CalendarFileInbox(Mutex<Vec<OpenedCalendarFile>>);

impl CalendarFileInbox {
    fn push(&self, file: OpenedCalendarFile) -> bool {
        let mut pending = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if pending.len() >= MAX_PENDING_CALENDAR_FILES {
            return false;
        }
        pending.push(file);
        true
    }

    fn take(&self) -> Vec<OpenedCalendarFile> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }
}

pub fn is_calendar_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ics"))
}

/// Reads and parses one file, checking its size before reading so a huge
/// file is never loaded. Failures become a readable error for the dialog.
fn read(path: &Path) -> OpenedCalendarFile {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Calendar invitation".into());
    let result = std::fs::metadata(path)
        .map_err(|_| "The file could not be read.".to_string())
        .and_then(|metadata| {
            if !metadata.is_file() {
                Err("This is not a calendar file.".to_string())
            } else if metadata.len() > MAX_CALENDAR_BYTES as u64 {
                Err("This calendar file is too large to preview.".to_string())
            } else {
                std::fs::read(path).map_err(|_| "The file could not be read.".to_string())
            }
        })
        .and_then(|bytes| calendar::parse(&bytes));
    match result {
        Ok(preview) => OpenedCalendarFile { name, preview: Some(preview), error: None },
        Err(error) => OpenedCalendarFile { name, preview: None, error: Some(error) },
    }
}

/// Reads an opened `.ics` file, queues it for the webview, and brings the
/// main window forward. Other files are ignored.
pub fn deliver<R: Runtime>(handle: &AppHandle<R>, path: &Path) {
    let Some(inbox) = handle.try_state::<CalendarFileInbox>() else {
        return;
    };
    if !is_calendar_file(path) {
        log::warn!("Ignored an opened file that is not a calendar invitation");
        return;
    }
    if !inbox.push(read(path)) {
        log::warn!("Ignored a calendar file because too many are waiting to be shown");
        return;
    }
    let _ = handle.emit(CALENDAR_FILE_EVENT, ());
    crate::focus_main_window(handle);
}

#[tauri::command]
pub fn take_pending_calendar_files(inbox: State<'_, CalendarFileInbox>) -> Vec<OpenedCalendarFile> {
    inbox.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVITE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:launch-review@example.com\r\nSUMMARY:Launch review\r\nDTSTART:20261012T150000Z\r\nDTEND:20261012T160000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("threestrands-calendar-files-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn recognizes_only_ics_files() {
        assert!(is_calendar_file(Path::new("/tmp/invite.ics")));
        assert!(is_calendar_file(Path::new("/tmp/INVITE.ICS")));
        for other in ["/tmp/invite.ics.exe", "/tmp/invite.vcs", "/tmp/invite", "/tmp/ics"] {
            assert!(!is_calendar_file(Path::new(other)), "{other}");
        }
    }

    #[test]
    fn reads_an_invitation_file() {
        let dir = temp_dir("read");
        let path = dir.join("Launch review.ics");
        std::fs::write(&path, INVITE).unwrap();
        let opened = read(&path);
        assert_eq!(opened.name, "Launch review.ics");
        assert_eq!(opened.error, None);
        let preview = opened.preview.unwrap();
        assert_eq!(preview.events[0].uid.as_deref(), Some("launch-review@example.com"));
        assert_eq!(preview.events[0].title, "Launch review");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reports_files_it_cannot_show() {
        let dir = temp_dir("errors");
        let missing = read(&dir.join("gone.ics"));
        assert_eq!(missing.error.as_deref(), Some("The file could not be read."));
        assert!(missing.preview.is_none());

        let folder = dir.join("folder.ics");
        std::fs::create_dir(&folder).unwrap();
        assert_eq!(read(&folder).error.as_deref(), Some("This is not a calendar file."));

        let garbage = dir.join("garbage.ics");
        std::fs::write(&garbage, "not a calendar").unwrap();
        assert!(read(&garbage).error.is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn enforces_the_calendar_file_size_limit() {
        let dir = temp_dir("size");
        // Pad the description so the file lands exactly on, then just over, the limit.
        let template = |padding: usize| INVITE.replace("SUMMARY:", &format!("DESCRIPTION:{}\r\nSUMMARY:", "x".repeat(padding)));
        let base = template(0).len();
        let exact = dir.join("exact.ics");
        std::fs::write(&exact, template(MAX_CALENDAR_BYTES - base)).unwrap();
        assert_eq!(std::fs::metadata(&exact).unwrap().len(), MAX_CALENDAR_BYTES as u64);
        assert_eq!(read(&exact).error, None);
        let over = dir.join("over.ics");
        std::fs::write(&over, template(MAX_CALENDAR_BYTES - base + 1)).unwrap();
        assert_eq!(read(&over).error.as_deref(), Some("This calendar file is too large to preview."));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn enforces_the_pending_queue_limit() {
        let inbox = CalendarFileInbox::default();
        let file = || OpenedCalendarFile { name: "invite.ics".into(), preview: None, error: Some("x".into()) };
        for _ in 0..MAX_PENDING_CALENDAR_FILES {
            assert!(inbox.push(file()));
        }
        assert!(!inbox.push(file()));
        assert_eq!(inbox.take().len(), MAX_PENDING_CALENDAR_FILES);
        assert!(inbox.take().is_empty());
    }
}
