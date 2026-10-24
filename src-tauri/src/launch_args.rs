//! Windows and Linux open a `mailto:` link or `.ics` file with ThreeStrands by
//! starting it with the link or path as an argument (the installer's ProgIDs
//! in windows/hooks.nsh and the desktop entry in linux/threestrands.desktop
//! pass it). The first copy reads its own arguments; a later launch is
//! forwarded to it by the single-instance plugin and exits, so opening a link
//! never starts a second copy over the same database. macOS delivers the same
//! links through `RunEvent::Opened` instead.
#![cfg_attr(target_os = "macos", allow(dead_code))]

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Runtime};

use crate::{calendar_files, mail_links};

#[derive(Debug, PartialEq, Eq)]
enum LaunchTarget {
    MailLink(String),
    CalendarFile(PathBuf),
}

/// What a launch asks to open. The first argument is the program itself, and
/// anything that is neither a mailto link nor an `.ics` file (as a path or a
/// `file:` URL, which desktop entries pass for `%U`) is ignored, including
/// other URLs that happen to end in `.ics`.
fn targets(args: &[String], cwd: &Path) -> Vec<LaunchTarget> {
    args.iter()
        .skip(1)
        .filter_map(|arg| {
            if mail_links::is_mailto(arg) {
                return Some(LaunchTarget::MailLink(arg.clone()));
            }
            // A one-letter scheme is a Windows drive, not a URL.
            let path = match url::Url::parse(arg) {
                Ok(url) if url.scheme() == "file" => url.to_file_path().ok()?,
                Ok(url) if url.scheme().len() > 1 => return None,
                _ => PathBuf::from(arg),
            };
            calendar_files::is_calendar_file(&path).then(|| LaunchTarget::CalendarFile(cwd.join(path)))
        })
        .collect()
}

/// Hands each link or file a launch asked for to the composer or the
/// invitation dialog.
pub fn deliver<R: Runtime>(handle: &AppHandle<R>, args: &[String], cwd: &Path) {
    for target in targets(args, cwd) {
        match target {
            LaunchTarget::MailLink(url) => mail_links::deliver(handle, &url),
            LaunchTarget::CalendarFile(path) => calendar_files::deliver(handle, &path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(rest: &[&str]) -> Vec<String> {
        std::iter::once("threestrands").chain(rest.iter().copied()).map(String::from).collect()
    }

    fn cwd() -> PathBuf {
        std::env::temp_dir().join("launch-args")
    }

    #[test]
    fn a_plain_launch_opens_nothing() {
        assert!(targets(&args(&[]), &cwd()).is_empty());
    }

    #[test]
    fn the_program_path_is_never_a_target() {
        let program = vec!["C:\\Program Files\\ThreeStrands\\invite.ics".to_string()];
        assert!(targets(&program, &cwd()).is_empty());
    }

    #[test]
    fn opens_mailto_links_as_given() {
        assert_eq!(
            targets(&args(&["mailto:jane@example.com?subject=Hi", "MAILTO:sam@example.com"]), &cwd()),
            vec![
                LaunchTarget::MailLink("mailto:jane@example.com?subject=Hi".into()),
                LaunchTarget::MailLink("MAILTO:sam@example.com".into()),
            ]
        );
    }

    #[test]
    fn resolves_calendar_files_against_the_launch_directory() {
        let absolute = cwd().join("elsewhere").join("Invite.ICS");
        assert_eq!(
            targets(&args(&["invite.ics", absolute.to_str().unwrap()]), &cwd()),
            vec![
                LaunchTarget::CalendarFile(cwd().join("invite.ics")),
                LaunchTarget::CalendarFile(absolute),
            ]
        );
    }

    #[test]
    fn accepts_windows_drive_paths() {
        let path = r"C:\Users\sam\Downloads\invite.ics";
        assert_eq!(targets(&args(&[path]), &cwd()), vec![LaunchTarget::CalendarFile(cwd().join(path))]);
    }

    #[test]
    fn accepts_calendar_files_as_file_urls() {
        let path = cwd().join("team offsite.ics");
        let url = url::Url::from_file_path(&path).unwrap();
        assert_eq!(
            targets(&args(&[url.as_str()]), &cwd()),
            vec![LaunchTarget::CalendarFile(path)]
        );
    }

    #[test]
    fn ignores_flags_other_files_and_other_schemes() {
        let ignored = [
            "--flag",
            "notes.txt",
            "invite.ics.exe",
            "https://example.com/invite.ics",
            "javascript:alert(1)",
            "file://",
            "",
        ];
        assert!(targets(&args(&ignored), &cwd()).is_empty());
    }
}
