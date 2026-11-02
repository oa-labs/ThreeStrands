# Phase 2: shared interactive workspace

Implemented on 2026-11-02 for 0.196.0. This completes the interactive workspace
milestone in [the delegated agents plan](../delegated-agents-plan.md), using the
existing deterministic native runner. It does not enable real research.

## User flow

Open **Agents** from the navigation rail or press **6**. Start with **New delegated
task** or **Shift+N** within Agents; **/** focuses its search. **Delegate Work from
Conversation** is also available in the command palette. The conversation context
panel and selected personal task expose **Delegate work…** and links to associated
delegated work.

The new-task form shows the account, objective, selected source conversation,
optional personal-task link, and the local demonstration disclosure. Execution
is disabled by default. **Web research** explains that the capability is unavailable
and cannot start a job. Enabling the demonstration makes no provider requests.

The list shows delegated work across accounts, with account labels, status,
unread activity, search, and unarchived, needs-input, finished, and archived views.
The detail displays conversation and result side by side, or accessible keyboard
tabs at narrow widths. Pause, resume, retry, cancel, archive, and confirmed deletion
use the native runner's existing state and budget checks.

Answering the required demonstration question resumes saved work. After completion,
**Send message** adds a demonstration discussion response without rewriting the
report. **Revise report** starts another bounded execution and publishes a new
immutable version. Earlier versions stay readable during execution and after
failure. A revision consumes the existing cumulative task allowance; it does not
reset it. Both native and demo implementations bound conversation length.

Personal-task links must belong to the same account. Linking, completion, unread
activity, and archiving never change personal-task status or email unread state.
Opening a missing source shows an unavailable message; reports remain readable.

## State and persistence

SQLite owns jobs, turns, and report versions. The App-level hook keeps selection,
unsent text, selected version/tab, and pane scroll offsets while switching
workspaces. Those view preferences and unsent text are session-local, not durable
drafts across app restarts.

The hook polls only while known work is queued or running, and refreshes on window
focus or explicit refresh. Polling reads state; it never queues or resumes work.
Mutation generations reject stale list responses. Background results update
activity indicators without taking focus or changing the selected job or draft.
Opening a task clears only the activity snapshot actually observed: native code
checks run generation, turn count, and artifact count atomically, preserving newer
unseen activity. Notifications stay inside the app.

The inherited list endpoint returns at most the newest 100 jobs. Pagination and
durable view preferences are not part of this preview.

## Boundaries and remaining work

Reports and conversation text render as plain text. HTML, Markdown images, frames,
and active content are not interpreted; no generated link opens an external site.
The sender-HTML iframe boundary is unchanged. No search, routing, model credentials,
external network tools, or paid calls are added. The preview explicitly labels
its reports as demonstrations that have not been researched.

Discussion is a fixed demonstration response, not a grounded model conversation.
Mid-run redirection, real provider setup, sources/citations, evidence-backed
follow-ups, invitation evaluation, and research profiles remain phases 3–4.
Existing phase-1 deletion, account scoping, recovery, and settings-transfer
exclusions continue to apply. Linking an existing personal task is supported;
creating a new decision task from a completed report remains future work.

## Verification

Automated coverage exercises delegation from mail, leaving and returning with an
unsent answer, clarification, discussion versus revision, version history, inert
markup, deliberate deletion, unavailable research, failed revision retention,
empty results, narrow tabs, stale reads, background completion, native account
link validation, cumulative budgets, and guarded unread updates.

Recorded checks:

- Frontend: 216 test files, 3,163 tests passed.
- Native: 2,058 passed, 7 existing ignored tests.
- Offline integration contracts: 31 passed; changelog contracts: 30 passed.
- Production build and ESLint passed.
- End-to-end: all 202 tests passed, including the complete Agents interaction flow.

Hands-on app verification remains the developer's responsibility.
