use std::io;
use std::path::Path;

use unicode_general_category::{get_general_category, GeneralCategory};
use unicode_normalization::UnicodeNormalization;

const DANGEROUS_EXTENSIONS: &[&str] = &[
    "action",
    "app",
    "appimage",
    "appref-ms",
    "appx",
    "application",
    "bat",
    "bash",
    "cab",
    "chm",
    "cmd",
    "com",
    "command",
    "cpl",
    "csh",
    "deb",
    "desktop",
    "dll",
    "dmg",
    "exe",
    "fish",
    "gadget",
    "hlp",
    "hta",
    "htm",
    "html",
    "img",
    "inf",
    "iso",
    "jar",
    "js",
    "jse",
    "jnlp",
    "ksh",
    "library-ms",
    "lnk",
    "mht",
    "mhtml",
    "msc",
    "msi",
    "msix",
    "msp",
    "pkg",
    "pl",
    "ps1",
    "psm1",
    "py",
    "rb",
    "reg",
    "rpm",
    "scf",
    "scpt",
    "scr",
    "settingcontent-ms",
    "sh",
    "svg",
    "terminal",
    "url",
    "vb",
    "vbe",
    "vbs",
    "vhd",
    "vhdx",
    "workflow",
    "wsf",
    "wsh",
    "xll",
    "xhtml",
    "zsh",
];

const MACRO_EXTENSIONS: &[&str] = &[
    "doc", "docm", "dot", "dotm", "pot", "potm", "ppam", "pps", "ppsm", "ppt", "pptm", "sldm",
    "xla", "xlam", "xls", "xlsb", "xlsm", "xlt", "xltm",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DetectedKind {
    ActiveDocument,
    Elf,
    Gif,
    Jpeg,
    MachO,
    Ole,
    Pdf,
    Pe,
    Png,
    Script,
    Zip,
}

impl DetectedKind {
    fn description(&self) -> &'static str {
        match self {
            Self::ActiveDocument => "active HTML or SVG document",
            Self::Elf | Self::MachO | Self::Pe => "executable program",
            Self::Script => "executable script",
            Self::Gif => "GIF image",
            Self::Jpeg => "JPEG image",
            Self::Ole => "legacy Microsoft Office document",
            Self::Pdf => "PDF document",
            Self::Png => "PNG image",
            Self::Zip => "ZIP archive",
        }
    }
}

pub(crate) fn normalize_filename(filename: &str) -> String {
    let normalized: String = filename.nfkc().collect();
    let cleaned: String = normalized
        .chars()
        .map(|character| {
            if matches!(
                get_general_category(character),
                GeneralCategory::Control
                    | GeneralCategory::Format
                    | GeneralCategory::LineSeparator
                    | GeneralCategory::ParagraphSeparator
            ) || matches!(character, '/' | '\\' | ':')
            {
                '_'
            } else {
                character
            }
        })
        .collect();
    let cleaned = cleaned
        .trim()
        .trim_start_matches('.')
        .trim_end_matches([' ', '.']);
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "attachment".to_string()
    } else {
        let cleaned = truncate_filename(cleaned, 240);
        let stem = cleaned.split('.').next().unwrap_or_default();
        let windows_reserved = matches!(
            stem.to_ascii_lowercase().as_str(),
            "con"
                | "prn"
                | "aux"
                | "nul"
                | "com1"
                | "com2"
                | "com3"
                | "com4"
                | "com5"
                | "com6"
                | "com7"
                | "com8"
                | "com9"
                | "lpt1"
                | "lpt2"
                | "lpt3"
                | "lpt4"
                | "lpt5"
                | "lpt6"
                | "lpt7"
                | "lpt8"
                | "lpt9"
        );
        if windows_reserved {
            format!("_{cleaned}")
        } else {
            cleaned
        }
    }
}

fn truncate_filename(filename: &str, max_bytes: usize) -> String {
    if filename.len() <= max_bytes {
        return filename.to_string();
    }

    let side_budget = (max_bytes - '…'.len_utf8()) / 2;
    let prefix_end = filename
        .char_indices()
        .take_while(|(index, character)| index + character.len_utf8() <= side_budget)
        .map(|(index, character)| index + character.len_utf8())
        .last()
        .unwrap_or_default();
    let suffix_start = filename
        .char_indices()
        .rev()
        .take_while(|(index, _)| filename.len() - index <= side_budget)
        .map(|(index, _)| index)
        .last()
        .unwrap_or(filename.len());
    format!("{}…{}", &filename[..prefix_end], &filename[suffix_start..])
}

pub(crate) fn opening_warnings(filename: &str, bytes: &[u8]) -> Vec<String> {
    let normalized_filename = normalize_filename(filename);
    let extension = Path::new(&normalized_filename)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    let mut warnings = Vec::new();

    if extension
        .as_deref()
        .is_some_and(|value| DANGEROUS_EXTENSIONS.contains(&value))
    {
        warnings.push(format!(
            ".{} files can run code or install software",
            extension.as_deref().unwrap_or_default()
        ));
    }
    if extension
        .as_deref()
        .is_some_and(|value| MACRO_EXTENSIONS.contains(&value))
    {
        warnings.push(format!(
            ".{} files can contain Microsoft Office macros",
            extension.as_deref().unwrap_or_default()
        ));
    }

    if let Some(kind) = detect_kind(bytes) {
        let expected_kinds = extension.as_deref().and_then(expected_kinds);
        if matches!(
            kind,
            DetectedKind::ActiveDocument
                | DetectedKind::Elf
                | DetectedKind::MachO
                | DetectedKind::Pe
                | DetectedKind::Script
        ) && expected_kinds.is_none()
            && !extension.as_deref().is_some_and(|value| {
                DANGEROUS_EXTENSIONS.contains(&value) || MACRO_EXTENSIONS.contains(&value)
            })
        {
            warnings.push(format!(
                "the file contents identify it as an {}",
                kind.description()
            ));
        }

        if expected_kinds.is_some_and(|expected| !expected.contains(&kind)) {
            warnings.push(format!(
                "the .{} extension does not match the detected {} contents",
                extension.as_deref().unwrap_or_default(),
                kind.description()
            ));
        }
    }

    warnings
}

pub(crate) fn opening_confirmation(filename: &str, bytes: &[u8]) -> Option<String> {
    let filename = normalize_filename(filename);
    let warnings = opening_warnings(&filename, bytes);
    (!warnings.is_empty()).then(|| {
        format!(
            "This attachment may be unsafe:\n\n• {}\n\nFile: {}\n\nOnly open it if you trust the sender and expected this file.",
            warnings.join("\n• "),
            filename
        )
    })
}

pub(crate) fn confirmation_allows_open(result: &rfd::MessageDialogResult) -> bool {
    result == &rfd::MessageDialogResult::Custom("Open anyway".into())
}

fn detect_kind(bytes: &[u8]) -> Option<DetectedKind> {
    let text_prefix = bytes
        .get(..bytes.len().min(512))
        .unwrap_or(bytes)
        .iter()
        .map(u8::to_ascii_lowercase)
        .collect::<Vec<_>>();
    let trimmed_text_prefix = text_prefix
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map(|start| &text_prefix[start..])
        .unwrap_or_default();

    if trimmed_text_prefix.starts_with(b"<!doctype html")
        || trimmed_text_prefix.starts_with(b"<html")
        || trimmed_text_prefix.starts_with(b"<svg")
        || trimmed_text_prefix
            .windows(4)
            .any(|window| window == b"<svg")
    {
        Some(DetectedKind::ActiveDocument)
    } else if bytes.starts_with(b"\x7fELF") {
        Some(DetectedKind::Elf)
    } else if bytes.starts_with(b"MZ") {
        Some(DetectedKind::Pe)
    } else if matches!(
        bytes.get(..4),
        Some([0xfe, 0xed, 0xfa, 0xce])
            | Some([0xfe, 0xed, 0xfa, 0xcf])
            | Some([0xce, 0xfa, 0xed, 0xfe])
            | Some([0xcf, 0xfa, 0xed, 0xfe])
            | Some([0xca, 0xfe, 0xba, 0xbe])
            | Some([0xbe, 0xba, 0xfe, 0xca])
    ) {
        Some(DetectedKind::MachO)
    } else if bytes.starts_with(b"#!") {
        Some(DetectedKind::Script)
    } else if bytes
        .get(..bytes.len().min(1024))
        .unwrap_or(bytes)
        .windows(5)
        .any(|window| window == b"%PDF-")
    {
        Some(DetectedKind::Pdf)
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(DetectedKind::Png)
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(DetectedKind::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(DetectedKind::Gif)
    } else if bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"PK\x05\x06")
        || bytes.starts_with(b"PK\x07\x08")
    {
        Some(DetectedKind::Zip)
    } else if bytes.starts_with(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1") {
        Some(DetectedKind::Ole)
    } else {
        None
    }
}

fn expected_kinds(extension: &str) -> Option<&'static [DetectedKind]> {
    match extension {
        "pdf" => Some(&[DetectedKind::Pdf]),
        "png" => Some(&[DetectedKind::Png]),
        "jpg" | "jpeg" => Some(&[DetectedKind::Jpeg]),
        "gif" => Some(&[DetectedKind::Gif]),
        "zip" => Some(&[DetectedKind::Zip]),
        "docx" | "xlsx" | "pptx" | "xlsb" => Some(&[DetectedKind::Zip, DetectedKind::Ole]),
        "doc" | "dot" | "xls" | "xla" | "xlt" | "ppt" | "pot" | "pps" => Some(&[DetectedKind::Ole]),
        value if MACRO_EXTENSIONS.contains(&value) => Some(&[DetectedKind::Zip, DetectedKind::Ole]),
        _ => None,
    }
}

pub(crate) fn quarantine(path: &Path) -> io::Result<()> {
    quarantine_platform(path)
}

#[cfg(target_os = "macos")]
fn quarantine_platform(path: &Path) -> io::Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let value = format!(
        "0083;{timestamp:x};ThreeStrands;{}",
        uuid::Uuid::new_v4().hyphenated()
    );
    xattr::set(path, "com.apple.quarantine", value.as_bytes())
}

#[cfg(target_os = "windows")]
fn quarantine_platform(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let mut stream = path.as_os_str().encode_wide().collect::<Vec<_>>();
    stream.extend(":Zone.Identifier".encode_utf16());
    let stream = std::ffi::OsString::from_wide(&stream);
    std::fs::write(stream, b"[ZoneTransfer]\r\nZoneId=3\r\n")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn quarantine_platform(path: &Path) -> io::Result<()> {
    // Freedesktop desktops do not define an enforcement mechanism equivalent
    // to Gatekeeper or SmartScreen. Preserve the download origin using the
    // metadata understood by Linux file managers instead of silently treating
    // the sender-controlled file as locally authored.
    let _ = xattr::set(path, "user.xdg.origin.url", b"email-attachment:");
    Ok(())
}

#[cfg(not(any(unix, target_os = "windows")))]
fn quarantine_platform(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "attachment quarantine is unsupported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_compatibility_characters_and_exposes_bidi_controls() {
        assert_eq!(
            normalize_filename("report\u{202e}fdp.ｅｘｅ"),
            "report_fdp.exe"
        );
        assert_eq!(normalize_filename("../bad\\name.pdf. "), "_bad_name.pdf");
        assert_eq!(
            normalize_filename("safe\u{2028}Verified by sender\u{feff}.txt"),
            "safe_Verified by sender_.txt"
        );
        assert_eq!(normalize_filename("NUL.txt"), "_NUL.txt");
        let long_name = format!("{}.ｅｘｅ", "a".repeat(300));
        let normalized = normalize_filename(&long_name);
        assert!(normalized.len() <= 240);
        assert!(normalized.ends_with(".exe"));
    }

    #[test]
    fn flags_dangerous_and_macro_enabled_extensions() {
        assert!(!opening_warnings("invoice.exe", b"MZ...").is_empty());
        assert!(opening_warnings("forecast.xlsm", b"PK\x03\x04...")
            .iter()
            .any(|warning| warning.contains("Office macros")));
        assert!(
            opening_warnings("legacy.doc", b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1")
                .iter()
                .any(|warning| warning.contains("Office macros"))
        );
    }

    #[test]
    fn catches_executable_content_hidden_behind_a_benign_extension() {
        let warnings = opening_warnings("notes.txt", b"#!/bin/sh\nrm -rf /");
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("executable script")));
        assert!(
            opening_warnings("drawing.txt", b"<svg><script>alert(1)</script></svg>")
                .iter()
                .any(|warning| warning.contains("active HTML or SVG"))
        );
    }

    #[test]
    fn checks_recognized_extension_signatures() {
        assert!(opening_warnings("document.pdf", b"%PDF-1.7\n").is_empty());
        assert!(opening_warnings("document.pdf", b"MZ...")
            .iter()
            .any(|warning| warning.contains("does not match")));
        assert!(opening_warnings("document.pdf", b"plain text").is_empty());
        assert!(opening_warnings("notes.txt", b"plain text").is_empty());
        assert!(opening_warnings("photo.png", b"\x89PNG\r\n\x1a\n").is_empty());
        assert_eq!(opening_warnings("INVOICE.EXE", b"MZ...").len(), 1);
        assert_eq!(
            opening_warnings("program", b"\x7fELF executable"),
            vec!["the file contents identify it as an executable program"]
        );
    }

    #[test]
    fn confirmation_is_absent_for_benign_files_and_leads_with_the_warning() {
        assert_eq!(opening_confirmation("notes.txt", b"plain text"), None);

        let confirmation =
            opening_confirmation("invoice\u{2028}Trusted by sender.ｅｘｅ", b"MZ...")
                .expect("unsafe attachment requires confirmation");
        assert!(confirmation.starts_with("This attachment may be unsafe:"));
        assert!(confirmation.contains(".exe files can run code"));
        assert!(confirmation.contains("File: invoice_Trusted by sender.exe"));
        assert!(!confirmation.contains('\u{2028}'));
    }

    #[test]
    fn only_the_explicit_open_anyway_action_allows_opening() {
        assert!(confirmation_allows_open(&rfd::MessageDialogResult::Custom(
            "Open anyway".into()
        )));
        assert!(!confirmation_allows_open(
            &rfd::MessageDialogResult::Custom("Cancel".into())
        ));
        assert!(!confirmation_allows_open(&rfd::MessageDialogResult::Cancel));
        assert!(!confirmation_allows_open(&rfd::MessageDialogResult::Ok));
        assert!(!confirmation_allows_open(&rfd::MessageDialogResult::Yes));
        assert!(!confirmation_allows_open(&rfd::MessageDialogResult::No));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn applies_macos_gatekeeper_quarantine() {
        let path =
            std::env::temp_dir().join(format!("threestrands-quarantine-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"attachment").unwrap();

        quarantine(&path).unwrap();

        let value = xattr::get(&path, "com.apple.quarantine")
            .unwrap()
            .expect("quarantine attribute");
        assert!(String::from_utf8(value).unwrap().starts_with("0083;"));
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn applies_freedesktop_download_origin() {
        let path =
            std::env::temp_dir().join(format!("threestrands-quarantine-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"attachment").unwrap();

        quarantine(&path).unwrap();

        if let Ok(Some(value)) = xattr::get(&path, "user.xdg.origin.url") {
            assert_eq!(value, b"email-attachment:");
        }
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn applies_windows_mark_of_the_web() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        let path =
            std::env::temp_dir().join(format!("threestrands-quarantine-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"attachment").unwrap();

        quarantine(&path).unwrap();

        let mut stream = path.as_os_str().encode_wide().collect::<Vec<_>>();
        stream.extend(":Zone.Identifier".encode_utf16());
        let value = std::fs::read(std::ffi::OsString::from_wide(&stream)).unwrap();
        assert!(value.windows(8).any(|part| part == b"ZoneId=3"));
        std::fs::remove_file(path).unwrap();
    }
}
