//! Persistence belongs in `db/` (see `docs/db-layer-plan.md`). The compiler
//! keeps the connection private to `db`; this scan covers the rest: no
//! production code outside `db/`, `db.rs`, and `schema.rs` may hold an SQL
//! statement literal or name `rusqlite`.
//!
//! Test code may still set up or inspect raw state, so the scan reads the
//! crate's module tree from `lib.rs` and `main.rs`, skips every item marked
//! `#[cfg(test)]`, and so never reads a file that only test code declares.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Counts {
    sql: usize,
    rusqlite: usize,
}

#[test]
fn persistence_stays_inside_the_db_module() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = BTreeMap::new();
    for root in ["lib.rs", "main.rs"] {
        walk(&src, &src.join(root), &mut found);
    }
    for expected in ["lib.rs", "correspondence.rs", "replicated_sync.rs", "provider/imap/mod.rs", "commands/accounts.rs"] {
        assert!(found.contains_key(expected), "the module walk never reached {expected}");
    }
    let problems: Vec<String> = found
        .iter()
        .filter(|(_, counts)| **counts != Counts::default())
        .map(|(path, counts)| {
            format!(
                "{path}: {} SQL literals, {} rusqlite references in production code. Add a named Database method in db/ instead.",
                counts.sql, counts.rusqlite
            )
        })
        .collect();
    assert!(problems.is_empty(), "Persistence code outside db/:\n{}", problems.join("\n"));
}

#[test]
fn the_scan_ignores_comments_and_counts_sql_in_every_literal_form() {
    let source = r####"
        // conn.execute("DELETE FROM drafts", []) in a comment
        /* "SELECT id FROM drafts" in a block comment */
        let quote = '"';
        let brace = '{';
        let imap = ["SELECT INBOX", "CREATE {mailbox}", "DELETE \"Archive\""];
        let plain = "SELECT id FROM drafts";
        let raw = r#"
            INSERT INTO drafts(id) VALUES (?1)
        "#;
        let escaped = "UPDATE drafts SET body = \"x\" WHERE id = ?1";
        use rusqlite::params;
    "####;
    assert_eq!(scan(source).0, Counts { sql: 3, rusqlite: 1 });
}

#[test]
fn the_scan_skips_test_items_and_finds_production_modules() {
    let source = r####"
        mod kept;
        #[path = "elsewhere/named.rs"]
        pub(crate) mod renamed;
        #[cfg(test)]
        mod only_tests;
        #[cfg(test)]
        use rusqlite::params;
        #[cfg(test)]
        mod tests {
            fn seed() { let sql = "DELETE FROM drafts WHERE id=?1"; let brace = "}"; }
        }
        fn production() { let sql = "SELECT id FROM drafts"; }
    "####;
    let (counts, modules) = scan(source);
    assert_eq!(counts, Counts { sql: 1, rusqlite: 0 });
    assert_eq!(
        modules,
        vec![("kept".to_string(), None), ("renamed".to_string(), Some("elsewhere/named.rs".to_string()))]
    );
}

/// Scans `file` and every production module it declares, recording counts
/// for each file outside `db/` and `schema.rs`.
fn walk(src: &Path, file: &Path, found: &mut BTreeMap<String, Counts>) {
    let relative = file.strip_prefix(src).unwrap().to_string_lossy().replace('\\', "/");
    if relative == "db.rs" || relative.starts_with("db/") || relative == "schema.rs" {
        return;
    }
    let source = std::fs::read_to_string(file).unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
    let (counts, modules) = scan(&source);
    found.insert(relative, counts);
    let directory = file.parent().unwrap();
    let stem = file.file_stem().unwrap().to_string_lossy();
    let children = if matches!(stem.as_ref(), "lib" | "main" | "mod") { directory.to_path_buf() } else { directory.join(stem.as_ref()) };
    for (name, path) in modules {
        let child: PathBuf = match path {
            Some(path) => directory.join(path),
            None if children.join(format!("{name}.rs")).exists() => children.join(format!("{name}.rs")),
            None => children.join(&name).join("mod.rs"),
        };
        walk(src, &child, found);
    }
}

/// Counts persistence access in `source`'s production code and lists the
/// file modules it declares, as `(name, #[path] value)`. Comments and
/// `#[cfg(test)]` items are skipped.
fn scan(source: &str) -> (Counts, Vec<(String, Option<String>)>) {
    let (code, literals) = tokenize(source);
    let (code, literals) = strip_test_items(&code, literals);
    let counts = Counts {
        sql: literals.iter().filter(|literal| is_sql(literal)).count(),
        rusqlite: code.matches("rusqlite").count(),
    };
    (counts, file_modules(&code, &literals))
}

/// Marks where each string literal was in the code.
const LITERAL: char = '\u{1}';

/// Splits source into code, with each string literal replaced by
/// [`LITERAL`] and comments and char literals removed, and the literals' text.
fn tokenize(source: &str) -> (String, Vec<String>) {
    let bytes = source.as_bytes();
    let mut code = String::new();
    let mut literals = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &source[i..];
        if rest.starts_with("//") {
            i += rest.find('\n').unwrap_or(rest.len());
        } else if rest.starts_with("/*") {
            i += rest.find("*/").map_or(rest.len(), |end| end + 2);
        } else if let Some(hashes) = raw_string_hashes(rest) {
            let open = 2 + hashes;
            let close = format!("\"{}", "#".repeat(hashes));
            let end = rest[open..].find(&close).map_or(rest.len(), |end| open + end);
            literals.push(rest[open..end].to_string());
            i += (end + close.len()).min(rest.len());
            code.push(LITERAL);
        } else if rest.starts_with('"') {
            let mut end = 1;
            while end < bytes.len() - i && bytes[i + end] != b'"' {
                end += if bytes[i + end] == b'\\' { 2 } else { 1 };
            }
            literals.push(rest[1..end.min(rest.len())].to_string());
            i += end + 1;
            code.push(LITERAL);
        } else if rest.starts_with("'\\") && rest.len() > 3 {
            i += rest[3..].find('\'').map_or(rest.len(), |end| end + 4);
        } else if rest.starts_with('\'') && rest.len() >= 3 && rest.is_char_boundary(2) && rest[2..].starts_with('\'') {
            // A one-byte char literal such as '"' or '{'.
            i += 3;
        } else {
            let ch = rest.chars().next().unwrap();
            code.push(ch);
            i += ch.len_utf8();
        }
    }
    (code, literals)
}

/// Removes every `#[cfg(test)]` item: up to its `;`, or through its braced
/// body, and the literals inside it.
fn strip_test_items(code: &str, literals: Vec<String>) -> (String, Vec<String>) {
    const ATTRIBUTE: &str = "#[cfg(test)]";
    let chars: Vec<char> = code.chars().collect();
    let mut kept_code = String::new();
    let mut kept_literals = Vec::new();
    let mut literal_index = 0;
    let mut i = 0;
    while i < chars.len() {
        if chars[i..].starts_with(&ATTRIBUTE.chars().collect::<Vec<_>>()) {
            let mut end = i + ATTRIBUTE.len();
            while end < chars.len() && chars[end] != ';' && chars[end] != '{' {
                end += 1;
            }
            if end < chars.len() && chars[end] == '{' {
                let mut depth = 0;
                while end < chars.len() {
                    match chars[end] {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    end += 1;
                }
            }
            let end = (end + 1).min(chars.len());
            literal_index += chars[i..end].iter().filter(|ch| **ch == LITERAL).count();
            i = end;
            continue;
        }
        if chars[i] == LITERAL {
            kept_literals.push(literals[literal_index].clone());
            literal_index += 1;
        }
        kept_code.push(chars[i]);
        i += 1;
    }
    (kept_code, kept_literals)
}

/// The file modules `code` declares (`mod name;`), with any `#[path]` value.
fn file_modules(code: &str, literals: &[String]) -> Vec<(String, Option<String>)> {
    let mut modules = Vec::new();
    let mut search = 0;
    while let Some(offset) = code[search..].find("mod ") {
        let start = search + offset;
        search = start + 4;
        let at_word_start = code[..start].chars().last().is_none_or(|ch| !(ch.is_alphanumeric() || ch == '_'));
        let rest = &code[start + 4..];
        let name: String = rest.trim_start().chars().take_while(|ch| ch.is_alphanumeric() || *ch == '_').collect();
        let after = rest.trim_start()[name.len()..].trim_start();
        if !at_word_start || name.is_empty() || !after.starts_with(';') {
            continue;
        }
        // A `#[path = "..."]` attribute directly before this declaration.
        let before = code[..start].trim_end();
        let before = before.strip_suffix("pub(crate)").or_else(|| before.strip_suffix("pub")).unwrap_or(before).trim_end();
        let path = before.strip_suffix(']').and_then(|head| {
            let attribute = &head[head.rfind("#[")?..];
            let compact: String = attribute.chars().filter(|ch| !ch.is_whitespace()).collect();
            (compact == format!("#[path={LITERAL}")).then(|| {
                let index = code[..start].rfind(LITERAL).map(|at| code[..at].matches(LITERAL).count()).unwrap();
                literals[index].clone()
            })
        });
        modules.push((name, path));
    }
    modules
}

/// The number of `#`s when `text` opens a raw string (`r"`, `r#"`, ...).
fn raw_string_hashes(text: &str) -> Option<usize> {
    let after_r = text.strip_prefix('r')?;
    let hashes = after_r.len() - after_r.trim_start_matches('#').len();
    after_r[hashes..].starts_with('"').then_some(hashes)
}

/// Whether a literal's text begins an SQL statement. IMAP commands share
/// keywords (`SELECT INBOX`, `CREATE {mailbox}`, `DELETE {mailbox}`), so each
/// keyword must appear in its SQL shape.
fn is_sql(text: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().collect();
    let second = words.get(1).copied().unwrap_or("");
    match words.first().copied().unwrap_or("") {
        "SELECT" => words.contains(&"FROM") || text.contains('('),
        "INSERT" | "REPLACE" => matches!(second, "INTO" | "OR"),
        "UPDATE" => words.contains(&"SET"),
        "DELETE" => second == "FROM",
        "CREATE" | "DROP" => {
            matches!(second, "TABLE" | "INDEX" | "UNIQUE" | "TRIGGER" | "VIEW" | "TEMP" | "TEMPORARY" | "VIRTUAL")
        }
        "ALTER" => second == "TABLE",
        "WITH" => words.contains(&"AS"),
        "PRAGMA" => !second.is_empty(),
        _ => false,
    }
}
