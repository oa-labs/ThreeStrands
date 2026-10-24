//! Lets the user make ThreeStrands the default for email links and calendar
//! invitations (Settings → Default Apps). See mail_links.rs and
//! calendar_files.rs for what happens when a link or file arrives.
//!
//! - macOS asks the user to confirm each change, so the app can request the
//!   role but never take it silently. Info.plist declares both roles.
//! - Windows lets only the user choose a default, so the installer registers
//!   ThreeStrands as a handler (windows/hooks.nsh) and the request opens its
//!   page in Windows Settings.
//! - Linux sets the default with `xdg-mime` for the desktop entry that
//!   launches this copy (linux/threestrands.desktop). An AppImage installs no
//!   entry, so one is written for it the first time it asks.

use serde::{Deserialize, Serialize};

/// The uniform type identifier macOS gives `.ics` files.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const ICS_CONTENT_TYPE: &str = "com.apple.ical.ics";

/// The name Windows lists ThreeStrands under in Default apps, and the ProgIDs
/// the installer registers for each role. Must match windows/hooks.nsh.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const WINDOWS_REGISTERED_APP: &str = "ThreeStrands";
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const WINDOWS_MAIL_PROG_ID: &str = "ThreeStrands.mailto";
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const WINDOWS_CALENDAR_PROG_ID: &str = "ThreeStrands.ics";

/// The freedesktop types for each role. The desktop entry must declare both.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const LINUX_MAIL_TYPE: &str = "x-scheme-handler/mailto";
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const LINUX_CALENDAR_TYPE: &str = "text/calendar";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DefaultAppRole {
    /// `mailto:` links.
    Mail,
    /// `.ics` calendar invitations.
    Calendar,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultAppStatus {
    /// False in development builds and copies that were not installed, which
    /// the system cannot register as a handler.
    pub supported: bool,
    pub mail: bool,
    pub calendar: bool,
    /// True when the user makes the change in the system's settings (Windows)
    /// rather than the app making it for them.
    pub opens_system_settings: bool,
}

const UNSUPPORTED: DefaultAppStatus = DefaultAppStatus {
    supported: false,
    mail: false,
    calendar: false,
    opens_system_settings: false,
};

const NOT_INSTALLED: &str = "Default apps can only be set from an installed copy of ThreeStrands.";

fn current_status() -> DefaultAppStatus {
    platform::status().unwrap_or(UNSUPPORTED)
}

/// Reading the status can mean scanning desktop entries or running a helper,
/// so it stays off the main thread.
#[tauri::command]
pub async fn default_app_status() -> DefaultAppStatus {
    tauri::async_runtime::spawn_blocking(current_status)
        .await
        .unwrap_or(UNSUPPORTED)
}

#[tauri::command]
pub async fn make_default_app(role: DefaultAppRole) -> Result<DefaultAppStatus, String> {
    platform::request(role).await?;
    Ok(default_app_status().await)
}

#[cfg(target_os = "macos")]
mod platform {
    use std::sync::Mutex;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSBundle, NSError, NSString, NSURL};
    use objc2_uniform_type_identifiers::UTType;

    use super::{DefaultAppRole, DefaultAppStatus, ICS_CONTENT_TYPE, NOT_INSTALLED};

    /// This app's bundle and identifier, or None when running outside a
    /// `.app` (as `tauri dev` does), where there is nothing to register.
    fn own_bundle() -> Option<(Retained<NSURL>, Retained<NSString>)> {
        let bundle = NSBundle::mainBundle();
        let identifier = bundle.bundleIdentifier()?;
        let url = bundle.bundleURL();
        url.path()?.to_string().ends_with(".app").then_some((url, identifier))
    }

    /// Compares by bundle identifier so a second copy of ThreeStrands (for
    /// example one still on a disk image) still counts as ThreeStrands.
    fn is_own(app: Option<Retained<NSURL>>, identifier: &NSString) -> bool {
        app.and_then(|url| NSBundle::bundleWithURL(&url))
            .and_then(|bundle| bundle.bundleIdentifier())
            .is_some_and(|other| other.to_string() == identifier.to_string())
    }

    fn ics_type() -> Option<Retained<UTType>> {
        UTType::typeWithIdentifier(&NSString::from_str(ICS_CONTENT_TYPE))
    }

    pub fn status() -> Option<DefaultAppStatus> {
        let (_, identifier) = own_bundle()?;
        let workspace = NSWorkspace::sharedWorkspace();
        let mail = NSURL::URLWithString(&NSString::from_str("mailto:"))
            .and_then(|probe| workspace.URLForApplicationToOpenURL(&probe));
        let calendar = ics_type().and_then(|ics| workspace.URLForApplicationToOpenContentType(&ics));
        Some(DefaultAppStatus {
            supported: true,
            mail: is_own(mail, &identifier),
            calendar: is_own(calendar, &identifier),
            opens_system_settings: false,
        })
    }

    pub async fn request(role: DefaultAppRole) -> Result<(), String> {
        let (sender, receiver) = tokio::sync::oneshot::channel::<Result<(), String>>();
        // The Objective-C objects are not Send, so they live only in this
        // block and are released before awaiting the user's answer.
        {
            let (app, _) = own_bundle().ok_or(NOT_INSTALLED)?;
            let sender = Mutex::new(Some(sender));
            let completion = RcBlock::new(move |error: *mut NSError| {
                // SAFETY: AppKit passes either null or a valid NSError that
                // lives for the duration of the completion handler.
                let result = match unsafe { error.as_ref() } {
                    None => Ok(()),
                    Some(error) => Err(error.localizedDescription().to_string()),
                };
                if let Some(sender) = sender.lock().ok().and_then(|mut sender| sender.take()) {
                    let _ = sender.send(result);
                }
            });
            let workspace = NSWorkspace::sharedWorkspace();
            match role {
                DefaultAppRole::Mail => workspace
                    .setDefaultApplicationAtURL_toOpenURLsWithScheme_completionHandler(
                        &app,
                        &NSString::from_str("mailto"),
                        Some(&completion),
                    ),
                DefaultAppRole::Calendar => {
                    let ics = ics_type().ok_or("macOS does not recognize calendar files")?;
                    workspace.setDefaultApplicationAtURL_toOpenContentType_completionHandler(
                        &app,
                        &ics,
                        Some(&completion),
                    );
                }
            }
        }
        receiver
            .await
            .map_err(|_| "macOS did not finish changing the default app".to_string())?
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::os::windows::ffi::OsStrExt;

    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
    };
    use windows::Win32::UI::Shell::{
        AssocQueryStringW, ASSOCF, ASSOCF_IS_PROTOCOL, ASSOCF_NONE, ASSOCSTR_PROGID,
    };

    use super::{
        DefaultAppRole, DefaultAppStatus, NOT_INSTALLED, WINDOWS_CALENDAR_PROG_ID,
        WINDOWS_MAIL_PROG_ID, WINDOWS_REGISTERED_APP,
    };

    fn wide(text: &str) -> Vec<u16> {
        std::ffi::OsStr::new(text).encode_wide().chain(std::iter::once(0)).collect()
    }

    /// Where the installer registered ThreeStrands, as the `ms-settings`
    /// parameter that opens its Default apps page: the machine for the
    /// per-machine installer, the user otherwise. None for a copy that was
    /// never installed.
    fn registration() -> Option<&'static str> {
        let key = wide("Software\\RegisteredApplications");
        let value = wide(WINDOWS_REGISTERED_APP);
        let registered = |root: HKEY| {
            // SAFETY: both strings are null-terminated and outlive the call,
            // and no data is requested back.
            let result = unsafe {
                RegGetValueW(root, PCWSTR(key.as_ptr()), PCWSTR(value.as_ptr()), RRF_RT_REG_SZ, None, None, None)
            };
            result == ERROR_SUCCESS
        };
        if registered(HKEY_LOCAL_MACHINE) {
            Some("registeredAppMachine")
        } else if registered(HKEY_CURRENT_USER) {
            Some("registeredAppUser")
        } else {
            None
        }
    }

    /// The ProgID that currently opens a scheme or extension, including the
    /// user's choice in Settings.
    fn current_prog_id(association: &str, flags: ASSOCF) -> Option<String> {
        let association = wide(association);
        let mut buffer = [0u16; 256];
        let mut length = buffer.len() as u32;
        // SAFETY: `association` is null-terminated, and `length` holds the
        // buffer's capacity in characters, which the call does not exceed.
        let result = unsafe {
            AssocQueryStringW(
                flags,
                ASSOCSTR_PROGID,
                PCWSTR(association.as_ptr()),
                PCWSTR::null(),
                Some(PWSTR(buffer.as_mut_ptr())),
                &mut length,
            )
        };
        if result.is_err() {
            return None;
        }
        let end = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..end]))
    }

    fn is_own(association: &str, flags: ASSOCF, prog_id: &str) -> bool {
        current_prog_id(association, flags).is_some_and(|current| current.eq_ignore_ascii_case(prog_id))
    }

    pub fn status() -> Option<DefaultAppStatus> {
        registration()?;
        Some(DefaultAppStatus {
            supported: true,
            mail: is_own("mailto", ASSOCF_IS_PROTOCOL, WINDOWS_MAIL_PROG_ID),
            calendar: is_own(".ics", ASSOCF_NONE, WINDOWS_CALENDAR_PROG_ID),
            opens_system_settings: true,
        })
    }

    /// Windows does not let an app take a default, so this opens the page
    /// where the user picks ThreeStrands for each role.
    pub async fn request(_role: DefaultAppRole) -> Result<(), String> {
        let parameter = registration().ok_or(NOT_INSTALLED)?;
        open::that(format!("ms-settings:defaultapps?{parameter}={WINDOWS_REGISTERED_APP}"))
            .map_err(|_| "Windows Settings could not be opened.".to_string())
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::{
        appimage_entry, desktop_entry_exec, exec_passes_targets, exec_program, DefaultAppRole,
        DefaultAppStatus, LINUX_CALENDAR_TYPE, LINUX_MAIL_TYPE, NOT_INSTALLED,
    };

    /// The desktop ID written for an AppImage, which installs no entry.
    const APPIMAGE_ENTRY_ID: &str = "app.threestrands.mail.desktop";

    fn data_home() -> Option<PathBuf> {
        std::env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
    }

    /// Where desktop entries live, most important first.
    fn application_dirs() -> Vec<PathBuf> {
        let system = std::env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        data_home()
            .into_iter()
            .chain(system.split(':').filter(|dir| !dir.is_empty()).map(PathBuf::from))
            .map(|dir| dir.join("applications"))
            .collect()
    }

    /// The program this copy runs as: the AppImage itself rather than the
    /// binary inside its temporary mount.
    fn own_program() -> Option<PathBuf> {
        std::env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .or_else(|| std::env::current_exe().ok())?
            .canonicalize()
            .ok()
    }

    fn resolve(program: &str) -> Option<PathBuf> {
        let found = if program.contains('/') {
            PathBuf::from(program)
        } else {
            let path = std::env::var_os("PATH")?;
            std::env::split_paths(&path).map(|dir| dir.join(program)).find(|candidate| candidate.is_file())?
        };
        found.canonicalize().ok()
    }

    /// The desktop ID of an installed entry that launches this copy and
    /// passes it the link or file being opened.
    fn own_entry() -> Option<String> {
        let own = own_program()?;
        application_dirs().into_iter().find_map(|dir| {
            std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
                let id = entry.file_name().into_string().ok()?;
                if !id.ends_with(".desktop") {
                    return None;
                }
                let contents = std::fs::read_to_string(entry.path()).ok()?;
                let exec = desktop_entry_exec(&contents)?;
                let launches_this_copy = exec_passes_targets(&exec)
                    && exec_program(&exec).and_then(|program| resolve(&program)).is_some_and(|program| program == own);
                launches_this_copy.then_some(id)
            })
        })
    }

    fn current_default(mime_type: &str) -> Option<String> {
        let output = Command::new("xdg-mime").args(["query", "default", mime_type]).output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub fn status() -> Option<DefaultAppStatus> {
        let entry = own_entry();
        if entry.is_none() && std::env::var_os("APPIMAGE").is_none() {
            return None;
        }
        let is_own = |mime_type: &str| {
            entry
                .as_deref()
                .is_some_and(|id| current_default(mime_type).as_deref() == Some(id))
        };
        Some(DefaultAppStatus {
            supported: true,
            mail: is_own(LINUX_MAIL_TYPE),
            calendar: is_own(LINUX_CALENDAR_TYPE),
            opens_system_settings: false,
        })
    }

    fn install_appimage_entry() -> Result<String, String> {
        let appimage = std::env::var_os("APPIMAGE").ok_or(NOT_INSTALLED)?;
        let contents = appimage_entry(Path::new(&appimage))
            .ok_or("This AppImage's location can't be used for a default app.")?;
        let dir = data_home().ok_or(NOT_INSTALLED)?.join("applications");
        std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(dir.join(APPIMAGE_ENTRY_ID), contents))
            .map_err(|_| "ThreeStrands could not add itself to your applications.".to_string())?;
        // Refreshes the desktop's cache where the tool exists; xdg-mime does
        // not need it.
        let _ = Command::new("update-desktop-database").arg(&dir).output();
        Ok(APPIMAGE_ENTRY_ID.to_string())
    }

    pub async fn request(role: DefaultAppRole) -> Result<(), String> {
        tauri::async_runtime::spawn_blocking(move || {
            let entry = match own_entry() {
                Some(entry) => entry,
                None => install_appimage_entry()?,
            };
            let mime_type = match role {
                DefaultAppRole::Mail => LINUX_MAIL_TYPE,
                DefaultAppRole::Calendar => LINUX_CALENDAR_TYPE,
            };
            let status = Command::new("xdg-mime")
                .args(["default", &entry, mime_type])
                .status()
                .map_err(|_| "Changing default apps needs xdg-utils, which is not installed.".to_string())?;
            if status.success() {
                Ok(())
            } else {
                Err("Your desktop did not accept the change.".to_string())
            }
        })
        .await
        .map_err(|_| "The default app could not be changed.".to_string())?
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod platform {
    use super::{DefaultAppRole, DefaultAppStatus};

    pub fn status() -> Option<DefaultAppStatus> {
        None
    }

    pub async fn request(_role: DefaultAppRole) -> Result<(), String> {
        Err("Default apps cannot be set on this system.".into())
    }
}

// Desktop entry reading and writing for Linux, kept outside its platform
// module so the format rules are tested everywhere. See the Desktop Entry
// Specification, "The Exec key".

/// The raw `Exec` value of the `[Desktop Entry]` group.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn desktop_entry_exec(contents: &str) -> Option<String> {
    let mut in_entry = false;
    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim() == "Exec" {
                    return Some(value.trim().to_string());
                }
            }
        }
    }
    None
}

/// Undoes the escapes every desktop entry string value may contain.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn unescape_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// The program an `Exec` value runs, with its quoting removed.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn exec_program(exec: &str) -> Option<String> {
    let exec = unescape_value(exec);
    let mut chars = exec.trim_start().chars().peekable();
    let mut program = String::new();
    if chars.peek() == Some(&'"') {
        chars.next();
        loop {
            match chars.next()? {
                '"' => break,
                '\\' => program.push(chars.next()?),
                c => program.push(c),
            }
        }
    } else {
        program.extend(chars.take_while(|c| !c.is_whitespace()));
    }
    let program = program.replace("%%", "%");
    (!program.is_empty()).then_some(program)
}

/// Whether an `Exec` value passes the opened link or file to the program.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn exec_passes_targets(exec: &str) -> bool {
    unescape_value(exec)
        .split_whitespace()
        .any(|argument| matches!(argument, "%u" | "%U" | "%f" | "%F"))
}

/// A hidden desktop entry that opens links and invitations with an AppImage,
/// or None for a path an `Exec` value cannot carry.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn appimage_entry(appimage: &std::path::Path) -> Option<String> {
    let path = appimage.to_str()?;
    if path.is_empty() || path.chars().any(char::is_control) {
        return None;
    }
    let mut quoted = String::from("\"");
    for c in path.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    // The quoted argument is itself a string value, whose backslashes are escaped.
    let exec = quoted.replace('\\', "\\\\");
    Some(format!(
        "[Desktop Entry]\nType=Application\nName=ThreeStrands\nExec={exec} %U\nTerminal=false\nNoDisplay=true\nMimeType={LINUX_MAIL_TYPE};{LINUX_CALENDAR_TYPE};\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_use_the_webview_names() {
        assert_eq!(serde_json::from_str::<DefaultAppRole>("\"mail\"").unwrap(), DefaultAppRole::Mail);
        assert_eq!(serde_json::from_str::<DefaultAppRole>("\"calendar\"").unwrap(), DefaultAppRole::Calendar);
        assert!(serde_json::from_str::<DefaultAppRole>("\"browser\"").is_err());
    }

    #[test]
    fn an_unbundled_build_reports_no_support() {
        // Tests run as a bare binary, never from a `.app`, an installer, or a
        // desktop entry.
        assert_eq!(current_status(), UNSUPPORTED);
    }

    #[test]
    fn the_bundle_declares_the_calendar_document_type() {
        let plist = include_str!("../Info.plist");
        assert!(plist.contains("<key>CFBundleDocumentTypes</key>"));
        assert!(plist.contains(&format!("<string>{ICS_CONTENT_TYPE}</string>")));
    }

    #[test]
    fn the_installer_registers_both_roles_without_taking_them() {
        let hooks = include_str!("../windows/hooks.nsh");
        assert!(hooks.contains(&format!(r#""mailto" "{WINDOWS_MAIL_PROG_ID}""#)));
        assert!(hooks.contains(&format!(r#"".ics" "{WINDOWS_CALENDAR_PROG_ID}""#)));
        assert!(hooks.contains(&format!(r#""Software\RegisteredApplications" "{WINDOWS_REGISTERED_APP}""#)));
        for prog_id in [WINDOWS_MAIL_PROG_ID, WINDOWS_CALENDAR_PROG_ID] {
            assert!(hooks.contains(&format!(r#"{prog_id}\shell\open\command" "" '"$INSTDIR\${{MAINBINARYNAME}}.exe" "%1"'"#)));
            assert!(hooks.contains(&format!(r#"DeleteRegKey SHCTX "Software\Classes\{prog_id}""#)));
        }
        // Only the user picks a default: never the extension's default value
        // or the user's choice.
        assert!(!hooks.contains(r#""Software\Classes\.ics" """#));
        assert!(!hooks.contains("UserChoice"));
        let config: serde_json::Value = serde_json::from_str(include_str!("../tauri.windows.conf.json")).unwrap();
        assert_eq!(config["bundle"]["windows"]["nsis"]["installerHooks"], "windows/hooks.nsh");
    }

    #[test]
    fn linux_packages_declare_both_roles_and_pass_the_target() {
        let template = include_str!("../linux/threestrands.desktop");
        assert!(template.contains(&format!("MimeType={LINUX_MAIL_TYPE};{LINUX_CALENDAR_TYPE};")));
        let exec = desktop_entry_exec(template).unwrap();
        assert!(exec_passes_targets(&exec));
        assert_eq!(exec_program(&exec).as_deref(), Some("{{exec}}"));
        let config: serde_json::Value = serde_json::from_str(include_str!("../tauri.linux.conf.json")).unwrap();
        for package in ["deb", "rpm"] {
            assert_eq!(config["bundle"]["linux"][package]["desktopTemplate"], "linux/threestrands.desktop");
        }
    }

    #[test]
    fn reads_exec_from_the_desktop_entry_group_only() {
        let entry = "[Desktop Action compose]\nExec=other --compose\n\n[Desktop Entry]\nName=ThreeStrands\nExec = threestrands %U\n";
        assert_eq!(desktop_entry_exec(entry).as_deref(), Some("threestrands %U"));
        assert_eq!(desktop_entry_exec("[Desktop Entry]\nName=ThreeStrands\n"), None);
    }

    #[test]
    fn finds_the_program_an_exec_value_runs() {
        assert_eq!(exec_program("threestrands %U").as_deref(), Some("threestrands"));
        assert_eq!(exec_program("/usr/bin/threestrands").as_deref(), Some("/usr/bin/threestrands"));
        assert_eq!(
            exec_program(r#""/home/sam/My Apps/ThreeStrands.AppImage" %U"#).as_deref(),
            Some("/home/sam/My Apps/ThreeStrands.AppImage")
        );
        assert_eq!(exec_program(r#""/unterminated %U"#), None);
        assert_eq!(exec_program("   "), None);
    }

    #[test]
    fn only_field_codes_for_links_or_files_pass_the_target() {
        for exec in ["app %u", "app %U", "app %f", "app %F"] {
            assert!(exec_passes_targets(exec), "{exec}");
        }
        for exec in ["app", "app %i", "app --u", "app 100%U"] {
            assert!(!exec_passes_targets(exec), "{exec}");
        }
    }

    #[test]
    fn an_appimage_entry_launches_that_appimage_hidden() {
        for path in [
            "/home/sam/Applications/ThreeStrands.AppImage",
            "/home/sam/My Apps/Three \"Strands\" $HOME `x` 100% \\ back.AppImage",
        ] {
            let entry = appimage_entry(std::path::Path::new(path)).unwrap();
            let exec = desktop_entry_exec(&entry).unwrap();
            assert_eq!(exec_program(&exec).as_deref(), Some(path), "{entry}");
            assert!(exec_passes_targets(&exec));
            assert!(entry.contains("NoDisplay=true\n"));
            assert!(entry.contains(&format!("MimeType={LINUX_MAIL_TYPE};{LINUX_CALENDAR_TYPE};\n")));
        }
        assert_eq!(appimage_entry(std::path::Path::new("/tmp/evil\nExec=rm.AppImage")), None);
        assert_eq!(appimage_entry(std::path::Path::new("")), None);
    }
}
