use super::*;
use crate::db::Database;

fn database() -> Database {
    Database::open_memory()
}

/// A folder for one test, removed when it ends.
struct TestFolder(PathBuf);

impl TestFolder {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("notes-import-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        TestFolder(path)
    }

    fn write(&self, relative: &str, contents: impl AsRef<[u8]>) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
}

impl Drop for TestFolder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest-of-image";

fn nothing() -> Existing {
    Existing {
        ids: HashSet::new(),
        titles: HashSet::new(),
    }
}

#[test]
fn a_single_file_uses_shared_title_rules_and_does_not_scan_its_neighbors() {
    let folder = TestFolder::new();
    folder.write(
        "chosen.MARKDOWN",
        "\u{feff}---\ntitle: \"Exact: title\"\n---\n\n# Kept heading\n\n**Body**\n",
    );
    folder.write("unrelated.md", [0xff]);
    let planned = plan_file(&folder.0.join("chosen.MARKDOWN"), &nothing()).unwrap();
    assert_eq!(planned.notes.len(), 1);
    assert_eq!(planned.notes[0].note.title, "Exact: title");
    assert_eq!(planned.notes[0].note.body, "# Kept heading\n\n**Body**\n");
    assert!(planned.problems.is_empty());
    for (name, contents, title, body) in [
        (
            "heading.md",
            "# Heading title\n\nBody\n",
            "Heading title",
            "Body\n",
        ),
        ("filename.md", "Body\n", "filename", "Body\n"),
        ("empty.md", "", "empty", ""),
    ] {
        folder.write(name, contents);
        let planned = plan_file(&folder.0.join(name), &nothing()).unwrap();
        assert_eq!(planned.notes[0].note.title, title);
        assert_eq!(planned.notes[0].note.body, body);
    }
}

#[test]
fn importing_one_file_always_adds_a_new_note_without_replacing_ids_or_daily_notes() {
    let folder = TestFolder::new();
    let db = database();
    let original = db.create_note("Existing", "Original body").unwrap();
    db.save_daily_note("2026-11-11", "Original daily body")
        .unwrap();
    folder.write(
        "daily/2026-11-11.md",
        format!(
            "---\nid: {}\ntitle: Existing\n---\n\nImported body\n",
            original.id
        ),
    );
    let path = folder.0.join("daily/2026-11-11.md");
    for expected_title in ["Existing 2", "Existing 3"] {
        let (ids, titles) = db.note_identities().unwrap();
        let planned = plan_file(&path, &Existing { ids, titles }).unwrap();
        let imported = &planned.notes[0].note;
        assert_ne!(imported.id, original.id);
        assert_eq!(imported.daily_date, None);
        assert_eq!(imported.title, expected_title);
        let id = imported.id.clone();
        let outcome = run(planned, |note, files| {
            db.import_note(note, files)
                .map_err(|error| error.to_string())
        });
        assert_eq!(outcome.notes, 1);
        assert!(outcome.problems.is_empty());
        let saved = db.get_note(&id).unwrap().unwrap();
        assert_eq!(saved.title, expected_title);
        assert_eq!(saved.body, "Imported body\n");
    }
    assert_eq!(
        db.get_note(&original.id).unwrap().unwrap().body,
        "Original body"
    );
    assert_eq!(
        db.get_daily_note("2026-11-11").unwrap().unwrap().body,
        "Original daily body"
    );
}

#[test]
fn single_file_import_rejects_invalid_text_types_and_oversized_notes_without_truncation() {
    let folder = TestFolder::new();
    for length in [
        MAX_NOTE_BODY_BYTES - 3,
        MAX_NOTE_BODY_BYTES - 2,
        MAX_NOTE_BODY_BYTES - 1,
    ] {
        let body = "a".repeat(length);
        folder.write("size.md", &body);
        let result = plan_file(&folder.0.join("size.md"), &nothing());
        if length <= MAX_NOTE_BODY_BYTES - 2 {
            assert_eq!(result.unwrap().notes[0].note.body, body);
        } else {
            assert!(result.unwrap_err().contains("Too long to sync"));
        }
    }
    folder.write("binary.md", [0xff]);
    assert!(plan_file(&folder.0.join("binary.md"), &nothing())
        .unwrap_err()
        .contains("Not readable as text"));
    folder.write("wrong.txt", "Body");
    assert!(plan_file(&folder.0.join("wrong.txt"), &nothing()).is_err());
    assert!(plan_file(&folder.0, &nothing()).is_err());
    assert!(plan_file(&folder.0.join("missing.md"), &nothing()).is_err());
    folder.write("with-large-file.md", "[packet](large.pdf)\n");
    fs::File::create(folder.0.join("large.pdf"))
        .unwrap()
        .set_len(MAX_NOTE_ATTACHMENT_BYTES + 1)
        .unwrap();
    assert!(plan_file(&folder.0.join("with-large-file.md"), &nothing())
        .unwrap_err()
        .contains("Larger than 20 MB"));
}

#[test]
fn a_single_file_attaches_local_files_but_keeps_remote_and_outside_links_unread() {
    let folder = TestFolder::new();
    let db = database();
    folder.write("notes/chosen.md", "# Chosen\n\n![](picture.png) [pdf](packet.pdf) ![](https://example.com/picture.png) [outside](../secret.pdf)\n");
    folder.write("notes/picture.png", PNG);
    folder.write("notes/packet.pdf", b"%PDF-1.4");
    folder.write("secret.pdf", b"private");
    let planned = plan_file(&folder.0.join("notes/chosen.md"), &nothing()).unwrap();
    let id = planned.notes[0].note.id.clone();
    assert_eq!(planned.notes[0].files.len(), 2);
    let outcome = run(planned, |note, files| {
        db.import_note(note, files)
            .map_err(|error| error.to_string())
    });
    assert_eq!((outcome.notes, outcome.files), (1, 2));
    let saved = db.get_note(&id).unwrap().unwrap();
    assert!(saved.body.contains("attachment:"));
    assert!(saved.body.contains("https://example.com/picture.png"));
    assert!(saved.body.contains("../secret.pdf"));
}

#[cfg(unix)]
#[test]
fn single_file_import_never_follows_symbolic_links() {
    let folder = TestFolder::new();
    folder.write("source.md", "# Source\n");
    std::os::unix::fs::symlink(folder.0.join("source.md"), folder.0.join("link.md")).unwrap();
    assert!(plan_file(&folder.0.join("link.md"), &nothing()).is_err());
    folder.write("chosen.md", "[linked](link.md) [secret](secret.pdf)\n");
    std::os::unix::fs::symlink("/etc/hosts", folder.0.join("secret.pdf")).unwrap();
    let planned = plan_file(&folder.0.join("chosen.md"), &nothing()).unwrap();
    assert!(planned.notes[0].files.is_empty());
    assert_eq!(
        planned.notes[0].note.body,
        "[linked](link.md) [secret](secret.pdf)\n"
    );
}

fn note<'a>(plan: &'a ImportPlan, title: &str) -> &'a PlannedNote {
    plan.notes
        .iter()
        .find(|planned| planned.note.title == title)
        .unwrap_or_else(|| panic!("no note titled {title}"))
}

/// A folder laid out as Reflect writes one.
fn reflect_folder() -> TestFolder {
    let folder = TestFolder::new();
    folder.write(
        "daily/2026-07-27.md",
        "---\nid: daily/2026-07-27.md\nsnippet: \"x\"\n---\n- Created [[OLLI - University of Pitt Program]] and [[PittsburghLeadershipNetworks]]\n",
    );
    folder.write("daily/notes-about-days.md", "# Not a day\n");
    folder.write(
        "notes/OlliProgram.md",
        "---\nid: Notes/OlliProgram\nsummary: \"OLLI\"\n---\n# OLLI - University of Pitt Program\n\nPhoto ID: ![](assets/pasted-1.png)\nPacket: [packet](../assets/packet.pdf \"Packet\") and again [same](assets/packet.pdf)\n",
    );
    folder.write(
        "notes/PittsburghLeadershipNetworks.md",
        "---\nid: 01kxp66n18p7vt6b5rsmd1taqy\naliases:\n  - Leadership customer network\nsummary: \"x\"\n---\n# Pittsburgh [Leadership] Networks\n\nSee [[leadership customer network]].\n",
    );
    folder.write(
        "collection/Reading.md",
        "---\nid: collections/Reading\n---\n# Reading\n\n- [The OLLI note](../notes/OlliProgram.md)\n- [OLLI - University of Pitt Program](../notes/OlliProgram.md)\n- [web](https://example.com/a.md)\n- [outside](../../secret.pdf)\n- [moved](../note/OlliProgram.md)\n\n```\n[[PittsburghLeadershipNetworks]] ![](assets/pasted-1.png)\n```\n",
    );
    folder.write(
        "person/barry.md",
        "---\nid: 01kxp66n18p7vt6b5rsmd1tbbb\n---\n# Personal\n",
    );
    folder.write("archive/old.md", "# Personal\n\nAn older one.\n");
    folder.write("person/bethany.md~other device", "# Bethany\n");
    folder.write("assets/pasted-1.png", PNG);
    folder.write(
        "assets/pasted-1.png.reflect.md",
        "---\nreflectAsset: true\n---\nA picture of an ID card.\n",
    );
    folder.write("assets/packet.pdf", b"%PDF-1.4");
    folder.write(".reflect/index.md", "# Hidden");
    folder
}

#[test]
fn a_reflect_folder_becomes_notes_with_titles_links_and_files() {
    let folder = reflect_folder();
    let plan = plan(&folder.0, &nothing()).unwrap();
    let summary = plan.summary();
    assert_eq!((summary.notes, summary.daily_notes), (6, 1));
    assert_eq!(summary.files, 2);

    let day = plan
        .notes
        .iter()
        .find(|planned| planned.note.daily_date.is_some())
        .unwrap();
    assert_eq!(
        (day.note.id.as_str(), day.note.title.as_str()),
        ("daily:2026-07-27", "2026-07-27")
    );
    // A title link stays; a file-name link follows the note to its title.
    assert_eq!(
        day.note.body,
        "- Created [[OLLI - University of Pitt Program]] and [[Pittsburgh -Leadership- Networks]]\n"
    );

    // The heading is the title and leaves the text; Reflect's keys go.
    let olli = note(&plan, "OLLI - University of Pitt Program");
    assert!(olli.note.id.starts_with("import-"));
    let [picture, packet] = &olli.files[..] else {
        panic!("{:?}", olli.files)
    };
    assert_eq!(
        (picture.relative.as_str(), packet.relative.as_str()),
        ("assets/pasted-1.png", "assets/packet.pdf")
    );
    assert_eq!(packet.declared_type, "application/pdf");
    // A second reading names the same files the same way, as another device would.
    let again = super::plan(&folder.0, &nothing()).unwrap();
    assert_eq!(
        note(&again, "OLLI - University of Pitt Program").note.body,
        olli.note.body
    );
    assert_eq!(
        olli.note.body,
        format!(
            "Photo ID: ![](attachment:{})\nPacket: [packet](attachment:{} \"Packet\") and again [same](attachment:{})\n",
            picture.id, packet.id, packet.id
        )
    );

    // Characters a title can't hold are replaced; aliases stay as front matter.
    let networks = note(&plan, "Pittsburgh -Leadership- Networks");
    assert_eq!(
        networks.note.body,
        "---\naliases:\n  - Leadership customer network\n---\n\nSee [[Pittsburgh -Leadership- Networks|leadership customer network]].\n"
    );

    // Relative links to notes become wiki links; code, web, and outside paths stay.
    let reading = note(&plan, "Reading");
    assert_eq!(
        reading.note.body,
        "- [[OLLI - University of Pitt Program|The OLLI note]]\n- [[OLLI - University of Pitt Program]]\n- [web](https://example.com/a.md)\n- [outside](../../secret.pdf)\n- [[OLLI - University of Pitt Program|moved]]\n\n```\n[[PittsburghLeadershipNetworks]] ![](assets/pasted-1.png)\n```\n"
    );
    assert!(reading.files.is_empty());

    // A clash gets a number; a file in `daily` that isn't a date is a regular note.
    note(&plan, "Personal");
    note(&plan, "Personal 2");
    note(&plan, "Not a day");
    assert_eq!(
        plan.problems
            .iter()
            .map(|problem| problem.file.as_str())
            .collect::<Vec<_>>(),
        ["person/bethany.md~other device"]
    );
}

#[test]
fn a_second_copy_of_a_note_is_imported_once_unless_it_changed() {
    let folder = TestFolder::new();
    folder.write("notes/a.md", "---\nid: same\n---\n# Retreat\n\nPlans\n");
    folder.write("notes/a 2.md", "---\nid: same\n---\n# Retreat\n\nPlans\n");
    folder.write("notes/b.md", "---\nid: other\n---\n# Meeting\n\nFirst\n");
    folder.write(
        "notes/b 2.md",
        "---\nid: other\n---\n# Meeting\n\nEdited since\n",
    );
    let plan = plan(&folder.0, &nothing()).unwrap();
    let mut titles: Vec<&str> = plan
        .notes
        .iter()
        .map(|planned| planned.note.title.as_str())
        .collect();
    titles.sort();
    assert_eq!(titles, ["Meeting", "Meeting 2", "Retreat"]);
    assert_eq!(plan.problems.len(), 1);
    assert!(plan.problems[0].reason.contains("imported once"));
    let ids: HashSet<&str> = plan
        .notes
        .iter()
        .map(|planned| planned.note.id.as_str())
        .collect();
    assert_eq!(ids.len(), 3);
}

#[test]
fn an_export_imports_back_with_its_ids_dates_and_files() {
    let folder = TestFolder::new();
    let id = "4f2a3c1e-8b7d-4e6f-9a0b-1c2d3e4f5a6b";
    folder.write(
        "notes/Floor plans.md",
        format!("---\nid: \"{id}\"\ncreated: \"2026-11-01T09:00:00Z\"\nupdated: \"2026-11-02T10:00:00Z\"\n---\n\n![](../attachments/Kitchen%20%281%29.png)\n"),
    );
    folder.write("attachments/Kitchen (1).png", PNG);
    let plan = plan(&folder.0, &nothing()).unwrap();
    let planned = &plan.notes[0];
    assert_eq!(planned.note.id, id);
    assert_eq!(planned.note.title, "Floor plans");
    assert_eq!(
        (
            planned.note.created_at.as_str(),
            planned.note.updated_at.as_str()
        ),
        ("2026-11-01T09:00:00Z", "2026-11-02T10:00:00Z")
    );
    assert_eq!(planned.files[0].name, "Kitchen (1).png");
    assert_eq!(
        planned.note.body,
        format!("![](attachment:{})\n", planned.files[0].id)
    );
}

#[test]
fn explicit_titles_preserve_body_headings_and_untitled_files_still_use_headings() {
    let folder = TestFolder::new();
    folder.write(
        "notes/file.md",
        "---\ntitle: \"Explicit: title\"\n---\n\n# Body heading\n\nText\n",
    );
    folder.write("notes/legacy.md", "# Heading title\n\nText\n");
    folder.write(
        "notes/fallback.md",
        "---\ntitle: \"\"\n---\n# Fallback title\n\nText\n",
    );
    let plan = plan(&folder.0, &nothing()).unwrap();
    assert_eq!(
        note(&plan, "Explicit: title").note.body,
        "# Body heading\n\nText\n"
    );
    assert_eq!(note(&plan, "Heading title").note.body, "Text\n");
    assert_eq!(note(&plan, "Fallback title").note.body, "Text\n");
}

#[test]
fn notes_and_titles_already_here_are_left_alone() {
    let folder = reflect_folder();
    let first = plan(&folder.0, &nothing()).unwrap();
    let existing = Existing {
        ids: first
            .notes
            .iter()
            .map(|planned| planned.note.id.clone())
            .collect(),
        titles: HashSet::new(),
    };
    let again = plan(&folder.0, &existing).unwrap();
    assert!(again.notes.is_empty());
    assert_eq!(again.already_here, 7);

    let taken = Existing {
        ids: HashSet::new(),
        titles: [title_key("Reading")].into_iter().collect(),
    };
    note(&plan(&folder.0, &taken).unwrap(), "Reading 2");
}

#[test]
fn notes_over_the_size_limit_are_reported_not_cut() {
    let folder = TestFolder::new();
    let at_limit = "a".repeat(MAX_NOTE_BODY_BYTES - 2);
    folder.write("notes/fits.md", &at_limit);
    folder.write("notes/over.md", format!("{at_limit}a"));
    folder.write(
        "notes/huge.md",
        "a".repeat(MAX_NOTE_FILE_BYTES as usize + 1),
    );
    folder.write("notes/binary.md", [0xff, 0xfe, 0x00]);
    let plan = plan(&folder.0, &nothing()).unwrap();
    assert_eq!(plan.notes.len(), 1);
    assert_eq!(plan.notes[0].note.body, at_limit);
    let mut problems: Vec<&str> = plan
        .problems
        .iter()
        .map(|problem| problem.file.as_str())
        .collect();
    problems.sort();
    assert_eq!(
        problems,
        ["notes/binary.md", "notes/huge.md", "notes/over.md"]
    );
}

#[test]
fn a_file_over_the_attachment_limit_keeps_its_link() {
    let folder = TestFolder::new();
    folder.write("notes/a.md", "# A\n\n[video](big.mov)\n");
    let file = fs::File::create(folder.0.join("notes/big.mov")).unwrap();
    file.set_len(MAX_NOTE_ATTACHMENT_BYTES + 1).unwrap();
    let plan = plan(&folder.0, &nothing()).unwrap();
    assert_eq!(plan.notes[0].note.body, "[video](big.mov)\n");
    assert!(plan.notes[0].files.is_empty());
    assert_eq!(plan.problems[0].file, "notes/big.mov");
}

#[cfg(unix)]
#[test]
fn nothing_outside_the_folder_is_read_through_a_link() {
    let outside = TestFolder::new();
    outside.write("secret.png", PNG);
    outside.write("elsewhere/note.md", "# Elsewhere\n");
    let folder = TestFolder::new();
    folder.write(
        "notes/a.md",
        "# A\n\n![](linked/secret.png) ![](../secret.png)\n",
    );
    std::os::unix::fs::symlink(&outside.0, folder.0.join("linked")).unwrap();
    std::os::unix::fs::symlink(
        outside.0.join("elsewhere"),
        folder.0.join("notes/elsewhere"),
    )
    .unwrap();
    let plan = plan(&folder.0, &nothing()).unwrap();
    assert_eq!(plan.notes.len(), 1, "a linked folder isn't read");
    assert!(plan.notes[0].files.is_empty());
    assert_eq!(
        plan.notes[0].note.body,
        "![](linked/secret.png) ![](../secret.png)\n"
    );
}

#[test]
fn importing_saves_notes_with_their_files_and_reports_what_it_couldnt() {
    let folder = reflect_folder();
    let db = database();
    let (ids, titles) = db.note_identities().unwrap();
    let plan = plan(&folder.0, &Existing { ids, titles }).unwrap();
    let olli_id = note(&plan, "OLLI - University of Pitt Program")
        .note
        .id
        .clone();
    let outcome = run(plan, |note, files| {
        db.import_note(note, files)
            .map_err(|error| error.to_string())
    });
    assert_eq!((outcome.notes, outcome.files), (7, 2));
    assert!(outcome.problems.is_empty(), "{:?}", outcome.problems);

    let olli = db.get_note(&olli_id).unwrap().unwrap();
    assert_eq!(olli.title, "OLLI - University of Pitt Program");
    assert_eq!(db.notes_storage().unwrap().attachment_count, 2);
    let links = db.note_links(&olli_id).unwrap();
    // The daily note once, and Reading on three lines.
    assert_eq!(links.backlinks.len(), 4);
    assert!(db.get_daily_note("2026-07-27").unwrap().is_some());

    // Reading the folder again finds everything here and imports nothing.
    let (ids, titles) = db.note_identities().unwrap();
    let again = super::plan(&folder.0, &Existing { ids, titles }).unwrap();
    assert_eq!((again.notes.len(), again.already_here), (0, 7));
}

#[cfg(unix)]
#[test]
fn a_file_that_became_a_link_after_reading_is_refused() {
    let db = database();
    let folder = TestFolder::new();
    folder.write("notes/b.md", "# B\n\n![](b.png)\n");
    folder.write("notes/b.png", PNG);
    let plan = plan(&folder.0, &nothing()).unwrap();
    fs::remove_file(folder.0.join("notes/b.png")).unwrap();
    std::os::unix::fs::symlink("/etc/hosts", folder.0.join("notes/b.png")).unwrap();
    let outcome = run(plan, |note, files| {
        db.import_note(note, files)
            .map_err(|error| error.to_string())
    });
    assert_eq!(outcome.notes, 0);
    assert_eq!(outcome.problems[0].file, "notes/b.md");
}

#[test]
fn a_note_starting_with_a_horizontal_rule_keeps_all_its_text() {
    let folder = TestFolder::new();
    let rules = "---\nSome text\n---\nMore text\n";
    folder.write("notes/rules.md", rules);
    folder.write("notes/empty-block.md", "---\n---\nBetween rules\n");
    folder.write(
        "notes/prose-after-key.md",
        "---\nsummary: x\nThis line is prose\n---\nBody\n",
    );
    folder.write("notes/unclosed.md", "---\nid: x\nno closing rule\n");
    folder.write(
        "notes/real.md",
        "---\nid: real\naliases:\n  - Other name\n\n- Second\n---\n# Real\n\nBody\n",
    );
    let plan = plan(&folder.0, &nothing()).unwrap();
    let body = |source: &str| {
        plan.notes
            .iter()
            .find(|planned| planned.source == source)
            .unwrap()
            .note
            .body
            .clone()
    };
    assert_eq!(body("notes/rules.md"), rules);
    assert_eq!(body("notes/empty-block.md"), "---\n---\nBetween rules\n");
    assert_eq!(
        body("notes/prose-after-key.md"),
        "---\nsummary: x\nThis line is prose\n---\nBody\n"
    );
    assert_eq!(body("notes/unclosed.md"), "---\nid: x\nno closing rule\n");
    // Real front matter still parses: its id is used and Reflect's keys go.
    let real = plan
        .notes
        .iter()
        .find(|planned| planned.source == "notes/real.md")
        .unwrap();
    assert_eq!(real.note.id, hashed_id("real"));
    assert_eq!(real.note.title, "Real");
    assert_eq!(
        real.note.body,
        "---\naliases:\n  - Other name\n\n- Second\n---\n\nBody\n"
    );
}
