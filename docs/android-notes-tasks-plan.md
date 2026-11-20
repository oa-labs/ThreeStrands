# ThreeStrands Android Notes and Tasks implementation plan

ThreeStrands v1 for Android opens to today's Daily Note and lets people write,
find notes, capture tasks, and complete them offline. It runs as a local-only
app from first launch. People who use ThreeStrands on desktop can also pair
their phone with an existing encrypted sync group without a mail or calendar
account. Keep the app in this repo and give Android its own release pipeline.
The shared Rust services and mobile UI can serve an iOS release later.

## The phone experience

Reflect Open's daily note, quick capture, and links between notes inform the
phone workflow. Use ThreeStrands typography, colors, and controls with
Android touch and navigation conventions. [Reflect Open on the App Store](https://apps.apple.com/us/app/reflect-open/id6787385615)
· [Reflect's iPhone workflow](https://reflect.app/blog/reflect-open)

| Screen | What v1 supports |
| --- | --- |
| **Daily** | Open today's existing note or show an empty editor for that date. Browse nearby days with a compact date strip and accessible previous, next, and Today controls. A **+** menu offers **Add to Daily Note**, **New Note**, and **New Task**. |
| **Notes** | Search titles and text offline, browse recent and starred notes, and create, edit, rename, star, trash, and restore notes. Follow existing note links and open backlinks from note details. Preserve attachment references and show their names. |
| **Tasks** | See open and completed work in a list. Create standalone tasks, edit their details and due dates, and complete or reopen them. A note task opens its source note at the task. Show synced mail-linked tasks with their available context and preserve the source link for desktop. |
| **Sync settings** | Pair from a desktop join code, see whether the last changes reached the connector, retry sync, and disconnect while keeping local data. Make local save status visible when a sync error occurs. |

In the editor, offer a touch toolbar with list, checklist, task, and note link
actions. Use the existing Markdown representation and preserve unfamiliar
markup, stable task markers, links, and attachment references through edits.
Square `[ ]` items stay in their note. Round `( )` items appear in Tasks and
can be checked off in either place. Creating a task **from the editor** adds
it to that note. **New Task** in the capture menu creates a standalone task.
The date of a Daily Note supplies context. Due dates are explicit on
standalone tasks. [ThreeStrands Notes and Tasks behavior](notes-design.md#markdown-checklists-and-tasks)

Follow the phone's local date for **Today** and keep the selected date
stable while the user writes, even across midnight or a time-zone change.
Keep the editor visible above the Android keyboard, support composition,
selection, and Android Back, and make tap targets and checked states clear
with large text and TalkBack. Use a simple Markdown edit and preview flow
initially, with checklist and task controls available by touch. Richer inline
formatting can follow once the editing path is proven on real keyboards.
Reuse the existing safe Markdown rendering and link-opening rules.

Later releases can add iOS, attachment uploads and previews, system share
capture, background notifications, Mail, Calendar, AI, and the desktop
Tasks board and Goals interface.

## Rust, Android, and the data boundary

- **Keep desktop and mobile in the same source tree.** Start with a separate
  Tauri 2 app under `apps/mobile/`, with a phone-specific React entry point,
  a small Rust app state, and a restricted command set. Leave the desktop
  app in its current location. Validate SQLite, the Rust build, and mobile
  WebView editing before committing to Tauri. If WebView editing fails the
  device acceptance tests, evaluate an Android-native editor or shell over
  the same Rust services. [Tauri mobile development](https://v2.tauri.app/develop/)
- **Extract the code as each vertical slice needs it.** Notes, note task
  indexing, and standalone Tasks currently live in `src-tauri/src/db/`.
  Their Tauri commands live in `src-tauri/src/commands/`, and the app-level
  sync runner is in `src-tauri/src/replicated_sync.rs`. Move the required
  SQLite operations, migrations, projections, and orchestration behind
  shared Rust entry points that desktop also uses. Reuse the existing
  protocol, envelope, merge, and transport crates. Keep Tauri events,
  scheduling, and app state at the platform boundary. Preserve the desktop
  schema and migrations, and reuse its SQLite and full-text search behavior
  on Android. [Rust workspace](../Cargo.toml) · [Sync engine design](replicated-sync-design.md#engine)
- **Share mutation paths.** Save notes with the existing revision check and
  note-task indexing transaction. Route task creation, updates, and completion
  through the same mutation and replication bookkeeping as desktop. Exercise
  backlog recovery for a newly created task. Also test interruption between
  updating an existing task and recording its change for replication. If
  needed, make the update and replica write atomic in the shared code. Use
  the Android app's private data directory for SQLite and recoverable drafts.
  Persist drafts during editing so process death can be handled on restart.
  Distinguish a stale local draft from a conflict between saved device
  versions. Give the user a way to compare versions and keep either version
  or both.
  [Note save and conflict behavior](notes-design.md#conflicts)
- **Keep the whole sync group compatible.** The existing format merges full
  encrypted snapshots. Android must retain desktop-only records, fields,
  tombstones, and attachment references through a pull, local edit, and push.
  Materialize the data needed by mobile and exercise the existing projection
  rules with a mixed desktop snapshot. Preserve the current unsupported-version
  behavior and tell users when an app update is needed. Keep the same device
  identity across in-place app upgrades.
  [Wire format and merge rules](replicated-sync-design.md#wire-format-protocol-3)
  · [Synced data and attachments](cross-device-sync.md#data-boundary)

## Pairing and recovery

Use **one HTTPS S3-compatible connector** for v1. A desktop can issue a join
code for its S3 connector while leaving its other connectors configured.
Preview the code before redeeming, and guide people with folder-only, IPFS,
or mixed-connector codes to create an S3-only code on desktop. Ask for S3
credentials if the inviter omitted them. Explain that pairing joins the
entire encrypted group and show when the inviter still needs to sync to
confirm the phone. Prove that work created locally before pairing survives
the join and reaches desktop. [Connectors and join codes](cross-device-sync.md#joining-a-sync-group)

Adapt the existing device identity, epoch key, and connector secret storage
to Android with a Keystore-backed adapter. Keep secrets out of SQLite,
WebView storage, logs, crash reports, and Android cloud or device-transfer
backups. Verify backup rules for the local database and drafts so app data
goes only where the user expects. Use the existing TLS validation, enrollment,
key rotation, and revocation rules. Distinguish expired invites, unavailable
credentials, lost keys, revoked devices, and connector failures. Provide a
local Markdown export and a way to preserve offline work before re-pairing
after key loss. [Android Keystore](https://developer.android.com/privacy-and-security/keystore)
· [Android backup rules](https://developer.android.com/identity/data/autobackup)

Sync on app launch and resume, periodically while open, and on **Sync Now**.
Reuse the existing serialized sync runner so triggers share a single in-flight
cycle. Preserve state during unavailable networks, failed uploads, and
process death. Refresh affected views after a merge without replacing unsaved
editor text or moving the user's selection. Once disconnected or revoked,
show that the device is local-only and preserve its work. [Existing sync cycle](../src-tauri/src/replicated_sync.rs)

## Build sequence and acceptance gates

1. **Prove the Android foundation.** Build a minimal Tauri Android app on an
   emulator and a physical ARM64 device. Open the existing SQLite schema in
   app-private storage, exercise full-text search, and save and reload a
   Daily Note. Write and recover a draft across process death. Prove that
   Keystore-backed material survives relaunch and an app upgrade. Test Gboard
   and a composing keyboard with selection, multiline text, and Android Back.
   Compile the shared sync dependencies and run a real HTTPS S3 probe before
   expanding the UI.
2. **Ship the local workflow to internal testers.** Implement Daily, Notes,
   and Tasks using the shared services. Add editor actions, local search,
   note-link navigation, Trash and restore, and recoverable drafts. Check
   that a task toggle edits the source Markdown once, keeps its stable marker,
   and gives immediate feedback in both views. Test airplane mode, app kill,
   date rollover, and accessible navigation.
3. **Prove two-way encrypted sync.** Pair with a current desktop build over
   S3. Create a round task and a square checklist offline in today's note.
   Reconnect and confirm desktop shows one task and the original checklist.
   Complete that task on desktop and verify the checkbox changes on Android.
   Repeat with a standalone task and a note rename. On both devices, exercise
   concurrent note edits, Trash and restore, key rotation and revocation,
   stale and reused join codes, and a dropped connection during upload.
   Verify that a desktop snapshot with other entity types and attachment
   references survives Android's edit and push.
4. **Distribute and release.** Run tests for the shared Rust workspace,
   the desktop Rust app and frontend, the mobile UI, and Android lifecycle
   and packaging. Use synthetic notes and tasks. Build a signed Android App
   Bundle in a separate CI workflow, choose the minimum Android version from
   device testing, and verify current Play target API, native 16 KB alignment,
   testing, and Data safety requirements. Start with a Play test track and
   test upgrading that installed build with local-only and paired data before
   promoting it. Publish setup, connector, and recovery instructions with
   the release. [Tauri Google Play distribution](https://v2.tauri.app/distribute/google-play/)
   · [Play target API policy](https://support.google.com/googleplay/android-developer/answer/16561298)
   · [Android 16 KB compatibility](https://developer.android.com/guide/practices/page-sizes)

The first implementation change should deliver the Android proof in step 1.
The public release depends on the existing desktop version and Android
converging through the same encrypted sync group after offline edits, with
every affected note, task, and desktop-only record still recoverable.
