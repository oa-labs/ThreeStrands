//! Persistence belongs in `db/` (see `docs/db-layer-plan.md`). This scan
//! counts SQL statement literals, `rusqlite` references, and direct
//! connection access in every source file outside `db/`, `db.rs`, and
//! `schema.rs`, and holds each file to the allowance below. The allowances
//! record what has not moved yet; they may only go down.

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Counts {
    sql: usize,
    rusqlite: usize,
    connection: usize,
}

/// Files still holding persistence code, as (path under `src/`, SQL
/// literals, `rusqlite` references, connection calls). Lower an entry when a
/// phase of the plan moves code into `db/`, and remove it at zero.
const ALLOWED: &[(&str, usize, usize, usize)] = &[
    ("correspondence.rs", 30, 1, 33),
    ("draft_push.rs", 12, 1, 12),
    ("enrollment.rs", 102, 3, 81),
    ("enrollment/error.rs", 0, 2, 0),
    ("enrollment/join_codes.rs", 22, 1, 21),
    ("lib.rs", 1, 0, 1),
    ("provider/imap/provider/tests/mod.rs", 13, 3, 10),
    ("replicated_sync.rs", 89, 1, 80),
    ("sync.rs", 7, 0, 7),
    ("sync_projection.rs", 20, 1, 14),
    ("sync_sim.rs", 7, 0, 6),
    ("transfer.rs", 1, 0, 1),
];

const CONNECTION_CALLS: [&str; 3] = [".connection()", ".with_connection(", ".with_transaction("];

#[test]
fn persistence_stays_inside_the_db_module() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = BTreeMap::new();
    collect(&src, &src, &mut found);
    assert!(found.contains_key("lib.rs"), "the scan read no sources from {}", src.display());

    let allowed: BTreeMap<&str, Counts> = ALLOWED
        .iter()
        .map(|&(path, sql, rusqlite, connection)| (path, Counts { sql, rusqlite, connection }))
        .collect();
    let mut problems = Vec::new();
    for (path, counts) in &found {
        let allowance = allowed.get(path.as_str()).copied().unwrap_or_default();
        if *counts == Counts::default() && !allowed.contains_key(path.as_str()) {
            continue;
        }
        for (what, actual, limit) in [
            ("SQL literals", counts.sql, allowance.sql),
            ("rusqlite references", counts.rusqlite, allowance.rusqlite),
            ("connection calls", counts.connection, allowance.connection),
        ] {
            if actual > limit {
                problems.push(format!(
                    "{path}: {actual} {what}, allowed {limit}. Add a named Database method in db/ instead."
                ));
            } else if actual < limit {
                problems.push(format!(
                    "{path}: {actual} {what}, allowance is {limit}. Lower its entry in ALLOWED."
                ));
            }
        }
    }
    for path in allowed.keys() {
        if !found.contains_key(*path) {
            problems.push(format!("{path}: no longer exists. Remove its entry from ALLOWED."));
        }
    }
    assert!(
        problems.is_empty(),
        "Persistence code outside db/ (see docs/db-layer-plan.md):\n{}",
        problems.join("\n")
    );
}

#[test]
fn the_scan_ignores_comments_and_counts_sql_in_every_literal_form() {
    let source = r####"
        // conn.execute("DELETE FROM drafts", []) in a comment
        /* "SELECT id FROM drafts" in a block comment */
        let quote = '"';
        let imap = ["SELECT INBOX", "CREATE {mailbox}", "DELETE \"Archive\""];
        let plain = "SELECT id FROM drafts";
        let raw = r#"
            INSERT INTO drafts(id) VALUES (?1)
        "#;
        let escaped = "UPDATE drafts SET body = \"x\" WHERE id = ?1";
        use rusqlite::params;
        database.with_connection(|c| Ok(()));
        database.connection()?;
    "####;
    assert_eq!(scan(source), Counts { sql: 3, rusqlite: 1, connection: 2 });
}

fn collect(root: &Path, dir: &Path, found: &mut BTreeMap<String, Counts>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let relative = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        if relative == "db" || relative == "db.rs" || relative == "schema.rs" {
            continue;
        }
        if path.is_dir() {
            collect(root, &path, found);
        } else if relative.ends_with(".rs") {
            found.insert(relative, scan(&std::fs::read_to_string(&path).unwrap()));
        }
    }
}

/// Splits source into code and string literals, skipping comments, and
/// counts each kind of persistence access.
fn scan(source: &str) -> Counts {
    let bytes = source.as_bytes();
    let mut code = String::new();
    let mut counts = Counts::default();
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
            counts.sql += usize::from(is_sql(&rest[open..end]));
            i += (end + close.len()).min(rest.len());
            code.push(' ');
        } else if rest.starts_with('"') {
            let mut end = 1;
            while end < bytes.len() - i && bytes[i + end] != b'"' {
                end += if bytes[i + end] == b'\\' { 2 } else { 1 };
            }
            counts.sql += usize::from(is_sql(&rest[1..end.min(rest.len())]));
            i += end + 1;
            code.push(' ');
        } else if rest.starts_with("'\\") && rest.len() > 3 {
            i += rest[3..].find('\'').map_or(rest.len(), |end| end + 4);
        } else if rest.starts_with('\'') && rest.len() >= 3 && rest.is_char_boundary(2) && rest[2..].starts_with('\'') {
            // A one-byte char literal such as '"'.
            i += 3;
        } else {
            let ch = rest.chars().next().unwrap();
            code.push(ch);
            i += ch.len_utf8();
        }
    }
    counts.rusqlite = code.matches("rusqlite").count();
    counts.connection = CONNECTION_CALLS.iter().map(|call| code.matches(call).count()).sum();
    counts
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
