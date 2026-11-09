# Notes workspace design

**Status:** Proposed, not built
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
| One field value | 64 KB (`MAX_VALUE_BYTES`) | `crates/sync-envelope/src/limits.rs` |
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
| `title` | the note's title; for a daily note, its date |
| `body` | the Markdown source, without front matter |
| `daily_date` | `YYYY-MM-DD` for a daily note, absent otherwise |
| `created_at` | when the note was created |
| `updated_at` | when the body or title last changed |
| `trashed_at` | when the note was moved to Trash, absent otherwise |

- **One field per concern.** Title, body, and trash state are separate
  fields, so renaming on one device while editing on another doesn't
  conflict.
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
  silently. Setting `trashed_at` keeps that edit recoverable. A permanent
  **Delete from Trash** is an ordinary replicated deletion.

### Size limits

- **Per note.** The encoded `body` value must fit in `MAX_VALUE_BYTES`. JSON
  escaping grows the text (newlines, quotes, and backslashes each take two
  bytes), so measure the encoded value, not the Markdown length. Put the
  note limit in one policy module with its own below-limit, exact-limit, and
  above-limit tests. A save that would pass the limit is refused, and the
  editor says the note is too long to sync and keeps the text locally. It
  never truncates.
- **All notes together.** Track notes' share of the snapshot. Warn as it
  approaches a budget well under 4 MB compressed, and refuse new notes
  before the snapshot as a whole would fail to seal. A failed seal would
  stop every other synchronized type from syncing too. **Settings → Notes**
  shows the usage.
- **The lasting fix** is splitting snapshots by entity type, already listed
  under the replicated sync design's known gaps. It is not part of v1.

### Write cadence

Every replicated write reseals and uploads the whole snapshot, so the editor
must not write one per keystroke.

- Save to the local `notes` table as the user types, so a crash loses
  nothing.
- Record the replicated write after about two seconds without typing, when
  the editor loses focus, when the user opens another note or workspace, and
  when the window closes.
- Opening or reading a note never writes. See the editor rules below.

### Local tables

All of these live on one device and are rebuilt from the replicated notes.
None of them travel through sync or settings export.

- `notes`: the materialized notes, written by the projection path like other
  synchronized types.
- `note_links`: one row per `[[link]]` per note, with the target title
  normalized (case folded and whitespace collapsed) and the target id once
  resolved.
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

Typing in a conflicted note keeps the conflict, because the new edit replaces
only this device's value. The banner stays until the user resolves it.

v1 doesn't merge concurrent text edits character by character. That would
need a text CRDT inside the field, which the snapshot model and the 64 KB
field limit don't fit.

## Workspace UI

Notes follows the layout and command rules in [DESIGN.md](../DESIGN.md#workspaces).

### Navigation

- Add `"notes"` to `RightWorkspace` in `src/useWorkspaces.ts`, so the active
  workspace stays one state value.
- `7` opens Notes (keys `1` through `6` are taken). Register it and the
  palette commands below in `src/commands.ts`, the central command registry:
  - **Go to Notes View** (`7`)
  - **Open Today's Note**
  - **New Note**
  - **Search Notes**
- Opening the workspace doesn't steal focus or open a form. It selects the
  last open note on this device, or today's note.

### Note list (left)

- **Today** is pinned at the top. Selecting it creates the daily note only
  once the user types, so browsing never writes an empty note.
- **Recent** comes next: notes opened on this device, newest first.
- A filter box searches titles as the user types. Searching inside bodies
  goes through **Search Notes** and `⌘K`.
- `j`/`k` and Up/Down move through the list, and Enter puts focus in the
  editor, as in Mail.
- **Trash** sits at the bottom. A trashed note opens read-only with
  **Restore** and **Delete Permanently**.

### Editor (center)

- The title is the first line, edited in place. A daily note's title is its
  date, shown with the weekday, and it can't be renamed.
- The body is the WYSIWYG Markdown editor described below.
- A quiet status line shows "Saved" or "Too long to sync", plus the conflict
  banner when there is one.

### Context panel (right)

Use `WorkspaceContextRail`, with `ContextSection` and `ContextRow` for every
list, under the panel rules in DESIGN.md: empty sections are left out, lists
show three rows before **Show more**, and headings collapse.

- **Backlinks**: notes that link here, each with the line around the link as
  its detail.
- **Unlinked**: `[[links]]` in this note whose target doesn't exist yet.
  Selecting one creates that note.
- **People**: `[[Name]]` links whose title matches a saved contact's name.
  Selecting one opens the contact in Contacts.

### Wiki links

- Typing `[[` opens a title picker with existing notes. Typing a new title
  and pressing Enter links to a note that doesn't exist yet.
- Links resolve by normalized title. Daily notes resolve by date
  (`[[2026-11-09]]`).
- Selecting a link opens its note, or creates it if missing.
- Renaming a note rewrites `[[Old Title]]` to `[[New Title]]` in every note
  that links to it, as ordinary edits on the renaming device. Each rewritten
  note counts toward the size limit, and one that would pass it keeps the old
  link and is listed in the rename's notice.

### Search

Note search uses the local FTS5 index. `⌘K` lists matching notes beside its
other results. **Search Current View** searches notes while Notes is open.

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
  the exact output.
- **No raw HTML.** Note text comes from other devices and from pastes, so it
  is untrusted. HTML in Markdown renders as text or is dropped, never as
  live markup.
- **Links open outside the app** through the opener plugin, after the same
  safety check mail links get.
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

## Security and privacy

- Notes are encrypted in snapshots like every other synchronized type.
  Storage providers see only ciphertext.
- Note text never goes into crash reports or diagnostics.
- v1 sends notes to no AI provider. If v2 adds "ask your notes", it needs
  its own consent, and a note-level private flag like Reflect's
  `private: true`.

## Tests

- **Rust:** two-device convergence for notes in `sync_sim`; daily notes
  created offline on two devices merge into one; edit racing Trash keeps the
  edit; edit racing permanent delete keeps the note deleted; conflict
  resolution through the shared command; encoded-size limit below, at, and
  above the limit; projection fills `notes`, `note_links`, and
  `note_search`; export file names, collisions, and folder containment.
- **Frontend:** the editor's round-trip fixtures; loading a note doesn't
  write; the write cadence; the conflict banner and its actions; rename
  rewriting links; the backlinks and unlinked sections. Test hooks and
  components directly, and keep one slim `<App />` test for the `7` shortcut
  and palette wiring.
- **Security-negative:** raw HTML, `javascript:` links, and remote image
  syntax in a note body never render live or fetch.

## Order of work

1. **Sync and storage.** The entity type, local tables, projection, search
   index, size limits, and daily-note ids, with the Rust tests above.
2. **Workspace shell.** The workspace, list, editor, write cadence, and
   today's note. Adopt the editor after its dependency review.
3. **Links.** Wiki links, backlinks, unlinked links, people, and rename
   rewriting.
4. **Safety.** The conflict banner and Trash.
5. **Export.** Export to Markdown Folder.

Each step ships with its tests. Steps 2 through 5 each add a changelog entry
under `### New` or `### Improved` as they become visible.

## Later

- **v2: Import a Reflect folder.** Read `daily/` and `notes/`, match front
  matter ids, and report notes over the size limit instead of truncating
  them.
- **v2: Folder mirror.** Keep a chosen folder in step with notes in both
  directions, for users who edit in other tools.
- Images and attachments, once snapshots are split by entity type or files
  travel separately.
- Tags beyond plain-text search.
- "Add to Today's Note" from a mail conversation, and linking notes to
  conversations, tasks, and events.
- Asking AI about notes, with a private flag per note.
- Merging concurrent text edits.
