//! Attachment file names made safe to show and save: compatibility forms
//! folded, control and bidirectional formatting characters exposed, path
//! separators replaced, length bounded, and Windows device names defused.

use unicode_general_category::{get_general_category, GeneralCategory};
use unicode_normalization::UnicodeNormalization;

pub fn normalize_filename(filename: &str) -> String {
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
}
