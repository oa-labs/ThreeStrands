# Notes workspace design

**Status:** v1 built: all five steps, with the meowdown editor, plus
[images and attachments](#images-and-attachments). v2 items are under
[Later](#later).
**Date:** 2026-11-09
**Scope:** a Notes workspace for Markdown notes that travel through
replicated sync

Notes adds a fourth kind of content beside mail, calendar, and tasks: the
user's own Markdown notes, with a daily note, `[[wiki links]]`, and
backlinks. It borrows its model from [Reflect](https://github.com/team-reflect/reflect-open),
an MIT-licensed, open-source notes app built on a folder of Markdown files.
The workspace is laid out like Mail: a list of notes on the left, the open
note in the middle, and the context panel on the right.

This document records the decisions for v1, what v1 leaves out, and the
order of work.

## Decisions

- **Notes are synchronized records, and Markdown is their text format.**
  Each note is one replicated entity whose body is the note's Markdown
  source. The user gets real files through **Export to Markdown Folder**,
  which writes Reflect's layout. A watched folder that is the source of
  truth, as in Reflect, is left for v2 (see [Later](#later)).
- **The editor is a WYSIWYG Markdown editor.** Reflect's editor hides
  Markdown syntax by default. It can also reveal syntax around the cursor or
  show it all the time. v1 matches that behavior; see [Editor](#editor) for
  the candidate library and the rules any editor must follow.
- **Importing an existing Reflect folder is v2.** The v1 export layout and
  front matter are chosen so a later import reads the same shape.

## Why records rather than a folder

Replicated sync is not file sync. Each device publishes one encrypted,
mergeable state that holds all of its synchronized data
([replicated sync design](replicated-sync-design.md)). A folder of files as
the source of truth would need a file watcher, reconciliation of edits made
by other editors, rename detection, and coexistence with whatever Dropbox or
iCloud does to the same folder. That is a second sync system beside the one
the app already has. Records keep one merge model, one conflict model, and
one encryption boundary, and the export still gives the user plain files
they own.

## Sync constraints

These limits shape most of the v1 design:

| Limit | Value | Where |
| --- | --- | --- |
| One field value | 96 KB (`MAX_VALUE_BYTES`) | `crates/sync-envelope/src/limits.rs` |
| One snapshot, compressed | 4 MB (`MAX_COMPRESSED_BYTES`) | same |
| One snapshot, decompressed | 32 MB (`MAX_DECOMPRESSED_BYTES`) | same |
| Republishing | every change reseals and uploads the whole snapshot | `replicated_sync.rs` |

Tasks, contacts, Split Inboxes, and every other synchronized type share the
snapshot budget with notes.

## Data model

### Replicated entity

Add `EntityType::Note` as the last variant in
`crates/sync-protocol/src/lib.rs`. The derived order is the snapshots'
canonical field order, so a new variant must go last.

| Field | Meaning |
| --- | --- |
| `_entity` | presence, as for every entity |
| `id` | the entity id |
| `title` | the note's title; for a daily note, its date |
| `body` | the Markdown source, without front matter |
| `dailyDate` | `YYYY-MM-DD` for a daily note, absent otherwise |
| `createdAt` | when a regular note was created; absent for a daily note |
| `trashed` | `true` while the note is in Trash |

Fields a later version adds are accepted and ignored, so it can add one
without older devices refusing its notes.

- **One field per concern.** Title, body, and trash state are separate
  fields, so renaming on one device while editing on another doesn't
  conflict.
- **No synchronized timestamps that change.** Two devices that touch the
  same note would always write different times, and the field would show as
  a conflict nobody cares about. So the time a note last changed, and when it
  went to Trash, are kept by each device for itself. Trash is the boolean
  `trashed`, so trashing on two devices at once agrees. A daily note has no
  `createdAt`, because its date says when it began and two devices starting
  the same day would otherwise disagree.
- **The body is one field.** Don't split a long body across several fields.
  Each field merges separately, so concurrent edits could combine halves of
  two versions into a note neither device wrote.
- **Daily notes share one ID.** A daily note's entity id is
  `daily:YYYY-MM-DD`. Two devices that open today's note while offline
  create the same entity, so they merge into one note, with at most a body
  conflict, rather than two duplicates. Other notes use a random id.
- **Trash is a field, not a deletion.** In this sync model a deletion beats
  a concurrent edit and keeps the entity deleted. If **Delete** removed the
  entity, an edit made on another device at the same time would be lost
  silently. Setting `trashed` keeps that edit recoverable. A permanent
  **Delete from Trash** is an ordinary replicated deletion. Deleting a note
  the replica already holds is recorded even while sync is off, so the note
  doesn't come back when sync resumes.
- **Titles.** A regular note's title is unique among notes outside Trash
  (ignoring case and repeated spaces), at most 200 characters, and can't
  contain `[`, `]`, or `|`, which end or split a link. It can't be a date,
  which names that day's daily note. A daily note's title is its date and
  can't be changed. If devices independently use the same title, the note
  with the lowest id keeps it and the others receive numbered titles, within
  the same 200-character limit. All authored titles are reserved before
  assigning numbers. This is a deterministic local projection, not a new
  replicated title edit: body edits never publish the generated name. Existing
  title-only links keep their text and resolve to the note keeping the original
  title; their intended target cannot be inferred across offline creations.

### Size limits

The limits live in `crates/sync-protocol` beside the other synchronized
types' limits.

- **Per note.** `MAX_NOTE_BODY_BYTES` is the envelope's `MAX_VALUE_BYTES`
  (96 KB, raised from 64 KB before Notes first shipped), measured on the body's JSON encoding (`note_body_bytes`), since
  that is what the envelope measures. Escaping grows the text (newlines,
  quotes, and backslashes each take two bytes), so a shorter body can be
  over. A save past the limit is refused with "This note is too long to
  sync"; the editor keeps the text and never truncates it.
- **All notes together.** `MAX_SYNCED_NOTES_TOTAL_BYTES` is 3.5 MiB (raised
  from 3 MiB before Notes first shipped), counted on the encoded fields of
  every note, Trash included. The snapshot holds at most 4 MB after
  compression, shared with every other synchronized type. Note text
  compresses well (a real 1.1 MB set of notes came to about 400 KB), and even
  base64 that encodes a pasted image or key compresses by about a quarter,
  so 3.5 MiB of notes stays under 2.7 MB compressed and leaves room for
  everything else. A change that would grow notes
  past it is refused; a change that doesn't grow its note always saves, so a
  user over the budget because of notes from another device can still trim.
  A failed seal would stop every other synchronized type from syncing too.
  **Settings → Notes** shows the usage (`notes_storage`).
- **The lasting fix** is splitting snapshots by entity type, already listed
  under the replicated sync design's known gaps. It is not part of v1.

### Write cadence

Every replicated write reseals and uploads the whole snapshot, so the editor
must not write one per keystroke.

As built, a text save (`update_note`, `save_daily_note`) records the change
in the replica but doesn't start a sync; the periodic sync (every 30
seconds) carries a burst of edits in one upload. Creating, trashing,
restoring, deleting, and importing notes still sync at once. Quitting runs
one more sync, bounded to 5 seconds (`finish_exit`); anything it doesn't
finish stays recorded and goes on the next launch.

- A save writes the `notes` row and the replica together, so the editor
  saves after about two seconds without typing, when it loses focus, when
  the user opens another note or workspace, and when the window closes.
- Until then the editor keeps the unsaved text itself (a per-device draft),
  so a crash loses at most what wasn't kept there.
- Saving text identical to what is stored writes nothing, and saving an
  empty body for a day without a note doesn't create it. Opening or reading
  a note never writes. See the editor rules below.
- Every save names the version it started from (the note's `updatedAt`, or
  none for a day without a note). If the note changed or was deleted on
  another device since, the save writes nothing and returns the note as it
  is now (`NoteSave { saved: false }`). Without this, a save would replace
  an edit that arrived while the user was typing, with no conflict to show
  for it, because the replica had already seen that edit. The editor then
  stops saving, keeps the user's text, turns read-only, and offers **Keep My
  Version** (save over it) or **Use Other Version** (drop the typing). For a
  deleted note the choices are **Keep as New Note** and **Discard My Text**.
- A note in Trash is never current, even though trashing changes no text
  and so leaves `updatedAt` alone: a save over it returns the trashed note
  instead of trying the edit. The editor then offers **Restore and Keep My
  Text** (restore, then save the typing over the restored note) or
  **Discard My Text** (show the note in Trash). If the restore is refused,
  such as when another note took the title, the reason shows and both
  choices stay. A draft left in a note that went to Trash while the app was
  closed asks the same way instead of retrying the save.
  A draft found on reopening that started from an older version gets the
  same choice.
- The app's exit waits for registered before-exit tasks (`beforeExit.ts`)
  after the composer saves its draft, so the open note saves before the app
  closes.

### Local tables

All of these live on one device and are rebuilt from the replicated notes.
None of them travel through sync or settings export.

- `notes`: the materialized notes, written by the projection path like other
  synchronized types. A local change writes the row and records its changed
  fields in the replica in one transaction. While sync is off, the changed
  field names wait in `replica_dirty_fields`; sync records them before it
  pulls, so they are concurrent with a peer's edits rather than replaced by
  them.
- `note_links`: one row per `[[link]]` per note (up to 500), with the target
  title normalized (case folded and whitespace collapsed) and the line it
  sits on. `[[Title|shown text]]` links to Title. Links inside fenced code
  blocks or inline code, and escaped ones (`\[[`), don't count.
- `note_search`: an FTS5 table over title and body, like `thread_search`.
- `note_visits`: when this device last opened each note, for the
  **Recent** list.

Recently opened order belongs to the device. Two devices show different
Recent lists, and that is intended.

### Compatibility

A device on an earlier version can't open a snapshot that holds a note. It
stops receiving changes from updated devices until it is updated, and
nothing is lost meanwhile. Add Notes to the data boundary in
[cross-device sync](cross-device-sync.md) with the release that ships it,
and tell users to update every device before writing notes, as for snooze
schedules.

### Settings export

Notes are content, not settings. They stay out of the encrypted settings
export, so the transfer format and its compatibility rules are not touched.
Export to Markdown Folder is how notes leave the app.

## Conflicts

Edits to the same note on two devices before they sync leave two values in
the `body` field. Every device shows the same working value, the one with
the greatest `(lamport, device, counter)`, and keeps the other for review.

Settings already lists sync conflicts under **Resolve Conflicts**, through
`FrontierConflictEditor`. That works for short values, but a note body is
too long to compare there. So v1 handles note conflicts in the note:

- A banner above a conflicted note says it was also edited on another
  device, and names the device.
- **Compare** shows the other version beside the current one, with changed
  lines marked.
- **Keep This Version** and **Use Other Version** resolve the conflict with
  an ordinary write, through the same command Settings uses.
- **Keep Both** writes the other version into a new note named
  "<title> (from <device>)", and keeps this one.
- The note list marks conflicted notes, and the generic Settings list links
  a note conflict to the note rather than rendering its whole body.
- With three or more devices in a conflict, the banner offers each other
  device's version in turn. A version is that device's value for every
  field in conflict and the shown value elsewhere.
- **Compare** aligns lines up to `MAX_DIFF_CELLS` line pairs; past that it
  marks everything between the shared start and end as changed.

A local write replaces every value of the field it writes, so typing into a
conflicted note would settle the conflict by discarding the other device's
version. The storage layer refuses an edit to a field in conflict ("This note
was also changed on another device. Choose which version to keep before
editing it."), so the editor stays read-only under the banner until the user
chooses. Resolving applies the chosen value on this device at once, as a
merge would.

v1 doesn't merge concurrent text edits character by character. That would
need a text CRDT inside the field, which the snapshot model and the 96 KB
field limit don't fit.

## Workspace UI

Notes follows the layout and command rules in [DESIGN.md](../DESIGN.md#workspaces).

### Navigation

- Add `"notes"` to `RightWorkspace` in `src/useWorkspaces.ts`, so the active
  workspace stays one state value.
- `4` opens Notes. Mail, Calendar, Tasks, and Notes are the core views, so
  they take `1` through `4`. Contacts, Files, and Agents are used less, and
  each moves up one key. The sidebar lists the views in key order:

  | Key | Before Notes | With Notes |
  | --- | --- | --- |
  | `0` | Today | Today |
  | `1` | Mail | Mail |
  | `2` | Calendar | Calendar |
  | `3` | Tasks | Tasks |
  | `4` | Contacts | **Notes** |
  | `5` | Files | **Contacts** |
  | `6` | Agents | **Files** |
  | `7` | — | **Agents** |

  The new keys ship in the same release as the Notes workspace, so `4` never
  opens an empty view and users relearn the keys only once. That release's
  changelog says the Contacts, Files, and Agents keys moved, and DESIGN.md's
  list of primary commands gets the new keys.
- Register the keys and the palette commands below in `src/commands.ts`, the
  central command registry:
  - **Go to Notes View** (`4`)
  - **Open Today's Note**, from anywhere
  - **New Note** (`Shift+N`), in Notes, like New Contact in Contacts
  - **Search Current View** (`/`) focuses the note search in Notes
- Opening the workspace doesn't steal focus or open a form. It selects the
  last open note on this device, or today's note.

### Note list (left)

- **Today** is pinned at the top. Selecting it creates the daily note only
  once the user types, so browsing never writes an empty note.
- **Recent** comes next: notes opened on this device, newest first. Each
  row's preview is the note's text without Markdown syntax (`plain_text`
  natively, `notePlainText` in the page, kept to the same cases).
- A search box above the list searches titles and note text as the user
  types, through the local search index, and lists the matches in place of
  the list. Escape clears it, then returns to the list.
- `j`/`k` and Up/Down move through the list, and Enter puts focus in the
  editor, as in Mail.
- **Trash** sits at the bottom. A trashed note opens read-only with
  **Restore** and **Delete Permanently**, which confirms first, with Cancel
  focused. Restoring a note whose title another note took meanwhile is
  refused until one is renamed. Today's note can go to Trash too; Today
  stays selected and shows it read-only.

### Editor (center)

- The title is the first line, edited in place. A daily note's title is its
  date, shown with the weekday, and it can't be renamed.
- The body is the editor described below, in `NoteEditor`, at the reading
  type size. Blocks sit close together, as in Reflect: list items
  `--space-1-5` apart, paragraphs `--space-2`, with more room above
  headings; the app's stylesheet overrides meowdown's 0.85em block gap.
- meowdown's menus are browser popovers that stay open while closed; the
  stylesheet hides closed ones and clears the browser's popover border,
  which otherwise shows as an empty box in the middle of the window.
- A button beside the title, and `#` in the list, move the note to Trash.
- A quiet status line shows "Saved" or "Too long to sync", plus the conflict
  banner when there is one.

### Context panel (right)

Use `WorkspaceContextRail`, with `ContextSection` and `ContextRow` for every
list, under the panel rules in DESIGN.md: empty sections are left out, lists
show three rows before **Show more**, and headings collapse. Step 2 shows
**Note details** (when the note was created, and last changed on this
device), and the sections below follow it.

- **Backlinks**: notes that link here, each with the line around the link as
  its detail.
- **Unlinked**: `[[links]]` in this note whose target doesn't exist yet.
  Selecting one creates that note.
- **People**: `[[Name]]` links whose title matches a saved contact's name.
  Selecting one opens the contact in Contacts. The first 30 distinct links
  are looked up, each against the contact search, matched by normalized
  name.

### Wiki links

- Typing `[[` opens a title picker with existing notes
  (`suggest_note_titles`: regular notes outside Trash whose title contains
  the typed text, prefix matches first, then recent). Typing a new title
  and pressing Enter links to a note that doesn't exist yet. meowdown also
  opens the picker for `@` at the start of a word, and inserts the same
  `[[Title]]` link.
- `[[Title|shown text]]` shows the shown text and follows Title.
- Links resolve by normalized title. Daily notes resolve by date
  (`[[2026-11-09]]`).
- Selecting a link opens its note, or creates it if missing.
- Renaming a note rewrites `[[Old Title]]` to `[[New Title]]` in every note
  that links to it, as ordinary edits on the renaming device, in the same
  transaction as the rename. Each rewritten note counts toward the size
  limit, and one that would pass it keeps the old link and is listed in the
  rename's notice (`NoteSave.relink`). So is a note whose body is in
  conflict: rewriting it would settle the conflict. Notes in Trash keep old
  links. A change of case or spacing only rewrites nothing, since links
  match by key.
- A title saves when the title field is left, not after the typing pause,
  so a rename rewrites links once rather than for every half-typed title.
  The typed title is kept in the draft meanwhile.

### Search

Note search uses the local FTS5 index. **Search Current View** searches
notes while Notes is open. Listing matching notes in `⌘K` beside its other
results is not built yet.

## Editor

### Reflect's editor

Reflect uses **meowdown** (`@meowdown/react`, `@meowdown/core`,
`@meowdown/markdown`), an MIT-licensed Markdown editor by the author of
ProseKit. It is built on ProseMirror through ProseKit, parses and writes
Markdown with remark, and uses CodeMirror's language package for code
blocks. Its "mark mode" has three settings, which Reflect exposes as
**Markdown syntax: Hide / Hybrid / Show**:

- **Hide**: full WYSIWYG, syntax never visible (Reflect's default).
- **Hybrid**: syntax appears only around the cursor, like Typora or
  Obsidian's live preview.
- **Show**: syntax always visible, still styled.

It also brings the `[[` menu, wiki-link hover cards, a slash menu, tags,
images, KaTeX math, and embeds.

### Candidate and review

meowdown is the first candidate, because it already does what Reflect
users expect. Before adopting it, run the [dependency
safety](../DEVELOPERS.md#dependency-safety) review, `cargo vet`, and
`cargo deny`, and note these concerns for the developer:

- **Churn.** It is pre-1.0, with about 160 releases since June 2026. The
  newest (0.84.0) is a day old, so pin an exact version that is at least two
  weeks old.
- **Weight.** It pulls in ProseKit, ProseMirror, the unified and remark
  stack, KaTeX, `@base-ui/react`, and embed packages. Measure the bundle
  growth, and turn off what v1 doesn't use: embeds that fetch remote content,
  math, and images.
- **Remote content.** Its embeds and link previews (YouTube, X posts) fetch
  from the network. v1 disables them. See the rules below.

If the review rejects meowdown, use ProseKit or Tiptap directly with a
remark-based Markdown parser and serializer. The rules below apply to any
choice.

**Adopted:** `@meowdown/react` 0.81.2 and `@meowdown/core` 0.81.3, pinned
exactly. Both, and `@meowdown/markdown` and `@meowdown/embed`, carry SLSA
provenance. Everything they add loads with the Notes chunk or later:
Notes itself is about 1 MB, Mermaid (1.5 MB) and KaTeX (256 KB) load only for
a diagram or math, and each code-block language loads on first use. The
startup bundle is unchanged. `NoteEditor` turns off every remote resolver
(images, X posts, YouTube) and embed paste; the app's CSP blocks remote
images and connections behind that. Upgrades follow the same rules: an exact
version at least two weeks old.

### Rules for any editor

- **Markdown text is the stored value.** The editor's document model is a
  view of `body`, never a second stored format.
- **Loading never writes.** A WYSIWYG editor normalizes Markdown when it
  parses and serializes (list markers, emphasis characters, blank lines).
  Write only after a user edit. Otherwise opening a note on two devices
  makes two different "edits" and a conflict nobody made.
- **Round-trip tests.** Fixtures for headings, nested lists, task lists,
  tables, code blocks, links, wiki links, and front matter must survive load
  and save unchanged. Where normalization is unavoidable, the test states
  the exact output. meowdown keeps all of them but tables, whose delimiter
  row it writes as `| --- |`; that happens only when the user edits the note.
- **No raw HTML.** Note text comes from other devices and from pastes, so it
  is untrusted. HTML in Markdown renders as text or is dropped, never as
  live markup.
- **Links open outside the app** through the opener plugin, after the same
  safety check mail links get: only `http`, `https`, and `mailto` open, and a
  click on a link never navigates the app's own window.
- **No remote fetches.** Remote images stay blocked in v1 and never load
  directly. Images in v2 must go through the existing native proxy and
  blocked-source flow, as in mail.
- **The editor reports its mode.** **Settings → Notes → Markdown Syntax**
  offers Hide, Hybrid, and Show, stored as a device preference.

## Export to Markdown Folder

**Settings → Notes → Export to Markdown Folder** writes every note that
isn't in Trash to a folder the user picks:

```text
<chosen folder>/
  daily/2026-11-09.md
  notes/<title>.md
```

- Titles become file names with characters that are unsafe on macOS,
  Windows, or Linux replaced. Clashes get a numeric suffix.
- Each file starts with front matter holding `id`, `created`, and `updated`,
  so a v2 import can match files back to notes.
- `[[wiki links]]` are written as typed, which Reflect and Obsidian both
  read.
- Export writes only inside the chosen folder, never follows symlinks out of
  it, and refuses a folder that isn't empty unless the user confirms
  replacing an earlier export there.

As built (`src-tauri/src/notes_export.rs`):

- The native folder picker's choice stays native-side; the page never sends
  a path, so an export writes only where the user pointed the picker.
- An export leaves `.threestrands-notes-export.json` listing the files it
  wrote. A folder holding only that list's export (and `.DS_Store` or
  `Thumbs.db`) can be replaced after confirming; replacing removes only the
  listed files that are still ordinary files inside the folder, and keeps
  anything else. A folder holding anything else is refused. The list is
  written before the notes, so an export that stops partway can still be
  replaced.
- Every file is created new (`create_new`), so nothing existing is
  overwritten or written through, including a symbolic link; a `daily` or
  `notes` that is a link or a file is refused.
- File names replace `/ \ : * ? " < > |` and control characters with `-`,
  replace a leading dot and trailing dots or spaces, add `-` to Windows
  device names, and are cut to 200 bytes (`MAX_FILE_STEM_BYTES`) at a
  character boundary. Clashes, ignoring case, get " 2", " 3", and so on.
- Front matter values are JSON-quoted strings: `id`, `created`, and
  `updated` as this device has them.

**Settings → Notes** also shows how much of the notes' sync space they use.

- Attachments go in `attachments/`, named by their file names with the same
  rules and clash numbering, and are listed in the manifest like notes. A
  note's `attachment:<id>` link destinations become
  `../attachments/<percent-encoded name>`; only a whole destination right
  after `](` changes. Files this device hasn't downloaded yet are left out,
  keep their `attachment:` links, and are counted in the result.

## Import Markdown Folder

**Settings → Notes → Import Markdown Folder** adds the notes in a folder the
user picks: a Reflect folder, or one Export to Markdown Folder wrote
(`src-tauri/src/notes_import.rs`).

- **Two steps.** Choosing the folder reads it and builds the whole plan
  without writing; the page sees a summary (notes, daily notes, files,
  notes already here, and what won't be imported, each with its reason) and
  imports only on **Import Notes**. As with export, the folder stays
  native-side.
- **Layout.** A `YYYY-MM-DD.md` in a folder named `daily` is that day's
  note; every other `.md` file, in any folder (Reflect's `notes`, `person`,
  `meetings`, `collection`, `archive`), is a regular note. Hidden entries
  (`.git`, `.reflect`) and symbolic links are skipped, and Reflect's
  `*.reflect.md` picture descriptions aren't notes. At most 20,000 entries
  and 5,000 notes are read.
- **Titles.** A leading `# Heading` is the title and leaves the text; else
  `title:`; else the file name. `[`, `]`, and `|` become `-`, a date gets
  " (note)", and a title in use (here or earlier in the folder) gets " 2",
  " 3", and so on.
- **Ids and re-importing.** A daily note's id is its date. A front matter
  `id` that is a ThreeStrands id (a UUID) is kept, so an export comes back
  as the same notes; any other `id`, or the path when there is none, is
  hashed into `import-<32 hex>`. A note whose id is already here is skipped
  and counted, so importing the same folder again adds nothing, and nothing
  is ever replaced. Two files with one `id` (iCloud's "name 2.md") are
  imported once when their text matches, and both, under different ids,
  when it doesn't.
- **Dates** come from `created` and `updated` when they are RFC 3339 times
  (an export's), else the file's modification time.
- **Links.** `[[name]]` naming a note's file name becomes `[[Title]]`, and
  one naming an alias becomes `[[Title|alias]]`. A relative link to a note
  file (`[text](../notes/Plan.md)`) becomes `[[Title|text]]`; one whose path
  went stale finds its note by file name when only one note has it. Fenced
  code, inline code, and web links are left alone.
- **Files.** A link or image whose destination is a file inside the folder,
  relative to the note or to the folder's root (Reflect writes
  `assets/…`), becomes an attachment, one per file per note. Nothing outside
  the folder, and nothing through a symbolic link, is read; each file is
  checked again when it is read for the import. A file over 20 MB keeps its
  link and is reported.
- **Front matter.** `id`, `title`, `created`, `updated`, `summary`,
  `snippet`, `summary_source_hash`, and `pinned` are dropped; anything else
  (such as `aliases`) stays at the top of the note.
- **Not imported**, with the reason shown: a note over 96 KB (never cut),
  a file that isn't UTF-8, Reflect's conflict copies (`name.md~other
  device`), and a matching second copy. A note that can't be saved during
  the import (notes out of sync space, a title taken meanwhile) is listed
  afterwards. Each note and its files are saved in one transaction
  (`import_note`), and recorded for sync like a new note.

## Images and attachments

Pasting or dropping a file into a note stores it and inserts
`![](attachment:<id>)` for an image or `[name](attachment:<id>)` for any
other file (meowdown's `onFilePaste` decides which). `<id>` is 32 hex
digits. The note's text holds only that reference, so a file never counts
toward the note's 96 KB or the notes' 3.5 MiB.

**Files travel separately from snapshots.** The sync design's whole-state
snapshots are resealed on every change, so carrying files in them would
re-upload every file on every keystroke's save. Instead:

- **The description is replicated.** `EntityType::NoteAttachment` holds
  `id`, `noteId`, `name`, `mimeType`, `size`, `sha256`, `createdAt`,
  `sealedBy` (the sealing device), and `index`, the CID of an object listing
  the file's sealed parts. Each description takes about 400 bytes of the
  snapshot, and notes hold at most `MAX_SYNCED_NOTE_ATTACHMENTS` (2,000)
  files. Like every new entity type, a device on an earlier version can't
  open a snapshot that holds one.
- **The bytes are sealed objects of their own.** The file is split into
  parts of at most `MAX_ATTACHMENT_PART_BYTES` (3 MiB, so a part that
  doesn't compress still fits a message), and each part is sealed as
  `ObjectKind::AttachmentPart`: chunked, padded, encrypted under the current
  epoch key, and signed by the sealing device under its own signature
  domain. Its message id is derived from the attachment id and part number,
  and opening checks both. A file holds at most
  `MAX_NOTE_ATTACHMENT_BYTES` (20 MiB).
- **Sealing waits for sync.** A file is recorded in the replica only once it
  is sealed, during the next push, so another device never learns of a file
  it can't fetch. The chunks go through the same delivery queue as
  snapshots, with `state_sequence` NULL, so they never hold up a head, and
  the repair sweep copies them to a newly added connector.
- **Downloading follows a pull.** A device fetches files it doesn't have,
  up to 64 MiB per cycle. Every object is checked against its CID, each part
  against its signer, attachment, and position, and the whole file against
  `size` and `sha256` before it is stored. A failure is retried after a
  minute, doubling to six hours. The sealing device's key is used whether
  or not that device was since removed: the objects are pinned by a snapshot
  an active device vouched for.
- **Deletion follows the note.** Deleting a note from Trash deletes its
  attachments on every device. Removing a reference from the text keeps the
  file, so undo and older versions still show it. The sealing device
  retires its objects through `sync_retired_objects`; they leave each
  connector once a snapshot without the attachment is published there.
- **Leaving a group** keeps the files this device has and seals them again
  for the next group; descriptions of files it never downloaded go.

**Showing files.** The editor's `resolveImageUrl` asks the native side for
an attached image as a `data:` URL, which the CSP already allows. A file is
an image only if its bytes are PNG, JPEG, GIF, WebP, or AVIF; the declared
type is never trusted, and SVG is never shown. The bytes are checked again,
with the mail image dimension limits, before each display. Remote image
sources still resolve to nothing. An image another device added waits and
appears once a sync brings it. Other files show as meowdown file pills with
their size. Clicking a file or image opens it in its default app through
the mail attachment path: the same warning for risky file types, a
quarantined copy in the reader cache.

**Settings → Notes** counts attached files and their size apart from the
notes' sync space.

**Files lists them too.** With `FileQuery::include_notes`, `list_files`
merges notes' files (outside Trash, today's unsaved note included) with mail
attachments, newest first by when each was sent or added: both lists are
read to the end of the requested page and the merged window is returned.
Files asks for them only while it covers every account, and the query
leaves them out when it names a conversation, sender, direction, the
calendar type, or a message to put first, since only mail has those. A
note's file carries `note` (id, title, daily date) instead of the mail
fields, and `availability` `syncing` until another device's file arrives.
Search matches the note's title as it matches a mail sender. Stars live in
`note_attachments.starred` (v102), on this device only. Previews, Open,
and Save as… go through the same native preview and quarantine paths as
mail attachments.

Known gaps:

- A file added to a note while another device permanently deletes that note
  outlives it until the note's device deletes it; nothing sweeps orphans.
- The sealing device keeps a sealed copy of each file (in `sync_objects`)
  so a new connector can be filled, so files take about twice their size on
  that device while sync is on.

## Security and privacy

- Notes are encrypted in snapshots like every other synchronized type.
  Storage providers see only ciphertext.
- Note text never goes into crash reports or diagnostics.
- v1 sends notes to no AI provider. If v2 adds "ask your notes", it needs
  its own consent, and a note-level private flag like Reflect's
  `private: true`.

## Tests

- **Rust:** two-device convergence for notes (direct merges, plus one
  push and pull through the fake transport with a body at the limit); daily notes
  created offline on two devices merge into one; edit racing Trash keeps the
  edit; edit racing permanent delete keeps the note deleted; conflict
  resolution through the shared command; encoded-size limit below, at, and
  above the limit; projection fills `notes`, `note_links`, and
  `note_search`; export file names, collisions, and folder containment.
- **Frontend:** the editor's round-trip fixtures; loading a note doesn't
  write; the write cadence; the conflict banner and its actions; rename
  rewriting links; the backlinks and unlinked sections. Test hooks and
  components directly, and keep one slim `<App />` test for the `4` shortcut
  and palette wiring. Existing App tests that press `4`, `5`, or `6`, or find
  the sidebar buttons "Contacts (4)", "Files (5)", and "Agents (6)", change
  with the new keys. That is a deliberate contract change.
- **Security-negative:** raw HTML, `javascript:` links, and remote image
  syntax in a note body never render live or fetch, even with attachments
  on; a file claiming an image type without an image's bytes is stored as
  `application/octet-stream` and never shown; a forged or corrupted part is
  refused.
- **Attachments:** a file of more than one part crosses the fake transport
  and arrives byte for byte; the snapshot carries only the description;
  deleting the note removes the objects from the connector; size and count
  limits below, at, and above; export file names and rewritten links.

## Order of work

1. **Sync and storage.** The entity type, local tables, projection, search
   index, size limits, and daily-note ids, with the Rust tests above.
2. **Workspace shell.** The workspace, list, editor, write cadence, today's
   note, and the new view keys. Built with the plain text area; adopt the
   WYSIWYG editor after its dependency review.
3. **Links.** Wiki links, backlinks, unlinked links, people, and rename
   rewriting.
4. **Safety.** The conflict banner and Trash.
5. **Export.** Export to Markdown Folder.

Each step ships with its tests. Steps 2 through 5 each add a changelog entry
under `### New` or `### Improved` as they become visible.

## Later

- **v2: Folder mirror.** Keep a chosen folder in step with notes in both
  directions, for users who edit in other tools.
- Tags beyond plain-text search.
- "Add to Today's Note" from a mail conversation, and linking notes to
  conversations, tasks, and events.
- Asking AI about notes, with a private flag per note.
- Merging concurrent text edits.
