# Task and calendar context rails, then Files

Status: proposed implementation plan, 2026-10-13. This document changes no application behavior.

## Product outcome

Users can archive an email containing useful attachments and find those files later, either by browsing Files or through context surfaced beside a task or meeting. Neither discovery path requires saving or organizing the attachment first. Starring an individual attachment is an optional importance signal.

Implement task and calendar workspace changes first, including useful automatic context, then ship Files using the same attachment query and row components. Do not require the Files workspace to exist before contextual files can appear.

## Layout and interaction decisions

- The established email context panel is the UI reference for right-side rails across Mail, Calendar, Tasks, Contacts, and Files. Extend that existing design language through shared components and styles. Workspace-specific content must still look and behave like the same application panel.
- Match the email panel's surface/background, dividing border, horizontal insets, typography, section spacing and separators, section headers/counts, icon sizes and alignment, row anatomy, trailing actions, truncation/tooltips, hover/focus states, and collapse affordances. Reuse existing design tokens and `ContextSection`, `ContextSectionHeader`, and `ContextRow` wherever applicable. New editable detail sections must use the same visual hierarchy and existing form/control styles.
- Use a standard resizable right rail. Treat the existing email panel's width as the starting reference; the proposed 360px default and 300–520px range are provisional and must align with the shared panel sizing convention rather than introduce a workspace-specific design. Reuse `useResizableWidth` and `PanelResizeHandle`; constrain the actual width to the workspace's available space. Keep preferred widths and collapsed state per device.
- The rail combines essential editable details and related content in one scrolling surface. Details appear first; Files, Conversations, and other useful sections follow. Share framing and section components, with workspace-specific content.
- Selecting a task or event updates its rail without stealing keyboard focus from the board or grid. Enter or the existing detail command focuses details; Escape returns to the originating item. Shortcuts must not intercept typing in fields.
- Preserve the selection when context loads, fails, or refreshes. Cancel or ignore stale requests after selection or account changes. Key draft edits by entity identity so changing selection cannot save one item's edits onto another.
- At narrower widths, the same details surface becomes a dismissible overlay with appropriate focus handling. The selected item remains visible and recoverable. Hideable context must remain reachable by keyboard.
- Do not shrink every workspace to accommodate multiple permanent right panels. Goals and calendar navigation remain accessible through compact controls described below.

## Existing foundations

| Area | Existing implementation | Planned reuse or change |
| --- | --- | --- |
| Task workspace | `src/TaskSidebar.tsx`, `src/GoalsPane.tsx` | Keep board/list, drag and keyboard movement, filters, goal workflows, and selection callbacks; replace workspace detail dialog with rail |
| Task editor | `src/TaskDetailDialog.tsx`, `src/TaskEditorDialog.tsx` | Extract reusable detail fields and save behavior; preserve dialog entry points outside Tasks |
| Primary calendar | `src/CalendarWeekView.tsx` | Generalize fixed seven-day grid and date range; replace right-side navigation with selected-event rail |
| Event details | `EventViewer` in `src/CalendarSidebar.tsx` | Extract event details/actions from popover dismissal and focus behavior; keep availability sidebar working |
| Context presentation | `src/ContextSections.tsx`, `src/contextPanelFocus.ts` | Reuse section headers, rows, collapse patterns, and keyboard conventions |
| Attachment history | `contact_files` in `src-tauri/src/db/contacts.rs` | Reuse attachment metadata and existing open/download paths; add account-scoped queries for workspace contexts and Files |
| Workspace routing | `src/App.tsx`, `src/useWorkspaces.ts`, `src/commands.ts` | Add Files after task/calendar delivery; preserve return navigation and existing workspace shortcuts |

The contact-files query currently collapses entries by filename and size. That is insufficient evidence of identical file content and must not become the Files identity or starring key. Preserve the existing contact contract while introducing a separate canonical attachment query.

## Phase 1: Task details and source context in the right rail

Deliverable: selecting a task shows its details and useful source material beside the board or list.

1. Extract task detail fields, validation, and save handling from `TaskDetailDialog` into reusable content. Keep status, title, notes, kind, due date/timezone, repeat behavior, goal selection, and source evidence intact.
2. Extract or reuse the email context panel's framing, tokens, and section components for the shared rail; separate its conversation-specific data and selection logic from its presentation. Integrate the rail into `TaskSidebar`; keep the existing board column minimums rather than increasing them. Preserve the established Mail and Contacts appearance during extraction.
3. Replace the permanent Goals pane with a compact Goals filter/control above the task content. It opens the existing goal list and management workflows in a drawer or popover. Preserve goal counts, supporting-goal filtering, review, stale indicators, account notices, creation, editing, and keyboard access. Selecting a filter must not replace task details with another permanent pane.
4. Add automatic source context: the source conversation, its non-inline attachments, source excerpt, and supporting goal. Prefer `sourceMessageId` when identifying the original material, while allowing the user to see files from the rest of the conversation. Standalone tasks remain fully functional with no source email.
5. Preserve autosave semantics with serialized writes, visible save failures, and retained unsaved edits. Selection changes must flush or retain pending edits safely; delayed save results must update the correct task only.
6. Keep quick-add in place and preserve modal task creation/editing from mail. Opening an email from the rail must support returning to the selected task, filter, layout, and scroll position.

Acceptance: task navigation updates details and source files automatically; edits persist; completing/moving a task, changing layout, and filtering do not corrupt selection or lose edits. Every existing goal action remains reachable.

## Phase 2: Calendar event rail and flexible date ranges

Deliverable: selecting an event shows essential details in the right rail while the calendar remains usable.

1. Extract event details/actions from `EventViewer`. In the primary calendar, render them in the rail rather than an outside-click-dismissed popover. Keep the availability sidebar's viewer as a wrapper around shared content.
2. Keep title, time/timezone, location, join link, response controls, and permitted edit/delete actions near the top. Show attendees when available. Make long descriptions expandable. Preserve current permissions, safe URL handling, delete confirmation, error handling, and schedule-cache invalidation.
3. Move mini-month and calendar selections into a header navigation control. Selected calendars and account scope remain visible and adjustable. The right rail has a helpful selection prompt when no event is selected.
4. Add explicit Day, 3-day, Workweek, and Week modes. Proposed initial default: Workweek. Persist the user's choice per device; resizing must not silently change it. Keep weekends accessible through Day, 3-day, and Week.
5. Derive grid columns, visible dates, query interval, heading, navigation increments, all-day rows, overlap placement, and drag-to-create calculations from one range model. Workweek advances by calendar weeks; 3-day advances three days; Day advances one day. Today selects today's date; on weekends Workweek shows the following Monday–Friday and makes that range explicit. Week retains the existing week-start convention.
6. When opening a specific event from mail, ensure its date is visible. If it falls outside Workweek, temporarily show Day without overwriting the user's saved range preference. Keep initial-event selection, timezone and DST behavior, and exclusive all-day end dates correct.
7. Keep event creation and full editing dialogs initially; successful saves refresh the selected event and context. Identify selection by account, calendar, and event ID, not event ID alone. Preserve selection through response updates and reloads while the event remains present.

Acceptance: all date ranges fetch and render their intended interval, event controls retain their current behavior, and changing days/ranges or opening from mail leaves selection and focus predictable.

## Phase 3: Automatic related context for tasks and meetings

Deliverable: both rails discover useful conversations and files without manual linking or AI configuration.

### Shared query and relevance contract

- Introduce typed context requests for task and meeting targets, with explicit mail-account scope. Mail and calendar accounts can differ; scope comes from the user's connected mail selection, never an assumption that a calendar address names a mail database account. Label cross-account results and exclude disconnected or unselected mail accounts.
- Return bounded Files and Conversations results with source identities, matching reasons, and separate loading/error states. Details stay usable when context cannot load. Deduplicate repeated appearances of the same attachment occurrence across evidence paths.
- Rank direct source relationships first. For tasks, use the source message/thread, task title/notes, and goal context. For meetings, use verified invitation identity if available, then topic evidence combined with attendees and a bounded time window. Attendee overlap alone belongs under clearly labelled recent history rather than high-confidence meeting materials.
- Centralize ranking weights, candidate limits, time windows, and tie breakers in one tested policy. Use local metadata and cached searchable text for the first release. No automatic attachment download, model call, or external upload is required to populate the panel.
- Explain inferred items with concise reasons such as “From source conversation” or “Recent conversation with attendees.” If there is insufficient evidence, show fewer results. Do not present inferred associations as confirmed links.
- Use debounced/cancelled requests, stable ordering, and bounded database queries. Do not load the entire mailbox into React or issue one history query per attendee.

### Data and implementation work

1. Add canonical attachment metadata/query types to `src/domain.ts` and `src/data/client.ts`, with equivalent native and demo implementations. Include account, message, thread, attachment ID, filename, type, size, timestamp, direction, and locally known availability.
2. Add a native attachment query module and shared context retrieval module. Start from existing message metadata; use bounded candidate sets for context retrieval. Define the API so Phase 4 can use the same identity and filtering rules with pagination.
3. Reuse existing local search and contact relationships for candidates. Invitation identity is not currently exposed in `ScheduleEvent`; add optional metadata only when reliably available, otherwise use the conservative fallback. Do not infer identity from a title alone.
4. Mount shared Files/Conversations sections in both rails. Extend task context beyond the direct source material shipped in Phase 1 and add meeting context to Phase 2's event rail.
5. Preserve the existing native attachment open/save path and email rendering boundary. Calendar descriptions and filenames remain untrusted text; opening a source email uses the normal reader.

Acceptance: fixtures cover a task with a source, a standalone task, a meeting with strong matches, unrelated correspondence from the same attendee, generic titles, multiple accounts, missing cached bodies, and rapidly changing selections. Results are deterministic and useful with AI disabled.

## Phase 4: Files workspace and attachment stars

Deliverable: files appear automatically across synchronized mail, and individual attachments can be starred consistently everywhere.

1. Add Files navigation and command-palette access, with `5` as the direct Files workspace shortcut. Display `5` in navigation tooltips and shortcut help, following the existing primary-workspace conventions. Do not activate it while typing in editable controls. Integrate workspace routing, lazy loading, account scope, and return navigation; extend command tests to cover registration, routing, and editable-control suppression.
2. Build a keyboard-navigable list with All and Starred, filename search, sender, date, type, account, and received/sent filters. Default to newest first with stable pagination. Archived mail remains included; Trash and Spam are excluded by default. Exclude inline resources from the primary list. Include calendar-file attachments with a type filter rather than inheriting the contact panel's `.ics` exclusion.
3. Show filename/type, sender or recipient, date, size, account, importance, and availability. Selecting a file uses the shared rail for metadata, source email, and actions. Reuse existing native viewing rather than introducing a new embedded document renderer.
4. Add an indexed attachment metadata projection for paginated browsing if needed by the query design. Backfill existing message attachments and maintain it transactionally during message upserts, replacement, deletion, retention, and account removal. Existing attachments must appear after upgrade without re-downloading mail or attachment bytes. Reuse the projection for context retrieval.
5. Persist stars separately from provider attachment JSON, keyed by account/message/attachment occurrence. Star/unstar must survive mailbox refresh and application restart. Never equate a filename and size with file identity, and never change the message's star as a side effect.
6. Add star actions to Files, message attachments, and contextual file rows, including contact file rows. Refresh all visible representations after mutation; failures restore prior state and report the error. Starred strengthens ordering among relevant context candidates; it does not make a file relevant to every task or meeting.
7. Keep attachment bytes on demand. A star does not promise offline retention. Show “Available offline,” “Download required,” or unavailable status only when supported by the existing cache/retrieval contract; add a metadata-only availability query if necessary.
8. First release: file stars are persistent on this device and explicitly described that way. They are not settings and are excluded from encrypted settings transfer. Cross-device annotation sync is a separate follow-up requiring portable provider identities and versioned protocol support; attachment bytes and inferred context remain device-local.
9. Clearly describe Files as covering mail synchronized on this device. Handle missing or deleted source messages with an unavailable state rather than silently opening another occurrence. Do not add a duplicate-grouping feature in this release.

Acceptance: untouched historical attachments appear; archiving does not remove a file; starring works across every surface and survives refresh/restart; account/filter changes paginate correctly; identically named same-size files remain distinct; offline/unavailable states are accurate.

## Validation and delivery

- Consistency is an acceptance criterion for every phase: rails use the shared email-derived framing, tokens, section headers, rows, and interaction conventions. Add component/integration assertions for shared composition and keyboard behavior; avoid brittle copies of CSS values. The developer verifies visual consistency against the existing email panel during hands-on review.
- Extend existing task, task-detail, calendar, attachment, contact-files, command, and return-navigation describes. Update dialog/popup expectations intentionally where the new workspace contract replaces them, while retaining assertions about edits, permissions, focus, and source navigation.
- Add shared rail resize/collapse/focus tests; task save-race and error-recovery tests; range/DST/weekend/all-day/overlap/drag tests; native account-scoping and relevance-negative tests; attachment projection migration/upsert/delete/pagination and star-persistence tests.
- Include malicious filenames/descriptions and attachment access tests. Context and Files must use existing sandboxed email rendering and native attachment protections.
- For each frontend change run `pnpm test`; for native changes run `cargo test` in `src-tauri`. Run `pnpm test:e2e` for changes touching triage, compose, or email rendering, including source-navigation integrations where applicable. Run the frontend build for integrated type checks.
- Add settings-transfer regression coverage if any portable preference schema changes. The proposed width, collapse, and calendar-range preferences stay per device and do not require such a change.
- Do not start the dev server or perform browser/screenshot QA. The developer owns hands-on evaluation of board density, calendar legibility, and rail width. Automated tests own behavior and layout-state contracts.
- Ship phases in the order above, splitting frontend and native work into reviewable commits as necessary. Each phase that ships new application functionality gets a minor version bump; fixes get patches. Determine the next version from the branch at implementation time and keep `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, and the `threestrands` entry in `src-tauri/Cargo.lock` identical.

## Follow-ups after the first release

Explicit pins and dismissals per meeting/task, direct goal materials, content-based duplicate grouping, full attachment-content search, semantic ranking, retained offline copies, and cross-device file annotations can build on these foundations. They are not prerequisites for automatic context or Files.
