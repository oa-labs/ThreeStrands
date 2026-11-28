# Inbox Assistants Design & Ideas

## Why old emails pile up

An inbox quietly gets used as four different tools at once: a **reading list**, a **reply queue**, a **task manager**, and a **temporary filing cabinet**. Once an email crosses a few days old, its psychological cost goes *up* (guilt, awkwardness, re-reading overhead) even as its real urgency usually goes *down*.

## What leading email clients do (and the gap they leave)

Most leading clients attack **incoming volume**, not **aging decay**:

- **HEY** — Prevents buildup up front with **The Screener**, separates reading into **The Feed** and receipts into **Paper Trail**, and splits active inbox debt into **Reply Later**, **Set Aside**, and **Bubble Up**.
- **Superhuman** — Optimizes velocity on new mail via **Split Inboxes**, **Auto Drafts** (pre-written replies in your voice), **Remind Me** (snooze, including "remind me if no reply"), and **Auto Archive** rules.
- **Shortwave** — Uses **Bundles**, **Scheduled Delivery**, and **AI thread summaries / Ghostwriter** to lower reading and reply friction.
- **Gmail** — Uses **Nudges** ("Received 5 days ago. Reply?" / "Sent 3 days ago. Follow up?") and Gemini thread summaries.
- **SaneBox** — Works upstream (**SaneLater**, **SaneNews**, **SaneBlackHole**) with a daily **Digest** and **SaneReminders** for unanswered sent mail.
- **Notion Mail / Spark** — Notion Mail leans on custom database-like **Views** and AI auto-labeling; Spark uses **Gatekeeper** and **Set Aside** / **Smart Inbox**.

**The gap:** Almost none of these actively help with **inbox decay**—emails the user *intentionally kept* a week or a month ago that are now expired, already resolved somewhere else, or stuck because the user never had 15 minutes to read or compose a thoughtful reply. Because ThreeStrands owns **Mail + Calendar + Tasks + Notes + Contacts** in one local SQLite database, it can solve inbox decay using cross-entity context rather than mail headers alone.

---

## Shipped v1: Standing Inbox Assistants

Delegated Tasks and separate Web Research settings were removed because they went unused. In their place, the **Agents** workspace (`7`) runs **standing, configurable Inbox Assistants** built on ThreeStrands' durable SQLite runner (`src-tauri/src/agents/`), the user's configured main AI provider, and a strict review-before-action proposal contract:

1. **Stale Reads Summarizer (`stale_reads`)**
   - *Target:* Unreplied Inbox threads older than *N* days that are newsletters, bulk updates/promotions, or long unread/unreplied emails.
   - *Action:* Summarizes the core takeaways (`takeaways` bullets, plus an optional `deadline` badge) with cited message evidence and offers **Archive**, **Save to Notes & Archive** (shown when `suggestSaveToNote` is set, evicting the email from Inbox while keeping the summary searchable in Notes), and **Create Task**.
2. **Expired & Already-Handled Sweeper (`resolved_and_expired`)**
   - *Target:* Aging Inbox threads that are either already resolved or past their useful window:
     - Deterministic zero-token check: the thread has no active (incomplete) task, and a linked ThreeStrands task completed after the email arrived with `completion_source = 'user'` and `completed_by_email IS NULL`.
     - Cross-entity AI verification: a newer thread with the same external participant where the user replied, a calendar event with the sender that already occurred, or an explicit deadline/event/promo date that has already passed.
   - *Action:* Proposes **Archive** with clickable resolving evidence (**View Completed Task**, **Reopen Task**, **Create Task** when no task is linked, or **View Resolving Thread**) and Undo, plus one-click bulk **Archive All N Safe** for `archive_as_expired` or `archive_as_resolved` backed by a user-completed task.
3. **Aging Reply Drafter (`aging_replies`)**
   - *Target:* Aging Inbox threads where the latest message is from an external human sender (excluding bulk/automated mail and threads that already have a draft).
   - *Action:* Drafts a natural reply for the user's review—plus an optional **Quick Check-In** angle when the thread is 5+ days old—and offers **Review Draft**, **Quick Check-In**, and **Archive**. Opens the ThreeStrands composer pre-filled for editing; never auto-sends.

4. **Sender Hygiene Advisor (`sender_hygiene`)**
   - *Target:* Senders, per account, that the user keeps ignoring: at least 3 quick dispositions (`isQuickTriageDisposition`: opened, archived or trashed from the Inbox in under a second, not batch, not scrolled) making up at least 60% of the sender's Inbox dispositions in the last 90 days, or at least 3 unread Inbox conversations whose last received message is older than the assistant's age setting. Excludes the account's own addresses, senders with a `response` or `restore` triage event in the window, senders the user has ever written to, senders already covered by a mute or block rule, and senders whose suggestion was dismissed in the last 30 days. Every threshold lives in `agents/policy.rs`.
   - *Action:* A deterministic, zero-token `sender_hygiene` proposal (no AI call, no AI log entry) anchored on the sender's newest Inbox conversation (preferring one that offers unsubscribing), with `senderEmail`, counts, and `inboxThreadIds` in the payload. Recommends **Unsubscribe** when a message offers it, otherwise **Mute…**, and also offers **New Split…** and **Archive N in Inbox**. Each action reuses the context panel's flow (`UnsubscribeConfirm`, `MuteSenderDialog`, `SenderSplitDialog`, undoable archive) and accepts the proposal when it succeeds; a new mute or block rule supersedes it. Never offered in the safe batch bar.

5. **Stale Reads Digest (`stale_reads_digest`)**
   - *Target:* Unread Inbox threads older than *N* days (default 7, up to 30 per run, oldest first, weekly default schedule) that are bulk reads (any message offers unsubscribing, or the thread carries `CATEGORY_UPDATES` / `CATEGORY_PROMOTIONS`) with no message from the user's own addresses. A thread with a pending `stale_reads` or `digest_item` proposal is skipped, and `stale_reads` in turn skips threads with a pending `digest_item`, so the two never propose the same thread.
   - *Action:* One per-thread `digest_item` proposal each, so supersede rules, per-item dismiss, and the Mail context-panel banner work unchanged; the **Agents** queue renders all pending items as a single card ("Stale Reads Digest · N unread reads") placed by its newest item. The card offers **Archive All N** (the undoable `onArchiveBatch` path; `digest_item` is never in the safe batch), **Dismiss All**, and per row **Save to Notes & Archive**, **Open Conversation**, and **Dismiss**. It is one keyboard stop: `e` archives all, `x` dismisses all, `o` / `Enter` opens the first item.
   - *Gists, AI-optional:* With a provider and model configured, one batched request per run (at most `DIGEST_ITEMS_PER_REQUEST` items each, bodies bounded by `DIGEST_ITEM_BODY_CHARS`) via `ai.fast_or_main_model()` (`AgentStaleReadsDigest`) returns `{candidateId, gist, suggestSaveToNote, deadline}` per item under a strict schema; unknown or duplicate IDs are ignored and an omitted or invalid item (including a gist over `DIGEST_GIST_MAX_CHARS`) keeps the deterministic gist, the thread preview trimmed to the same limit. Without a provider, or if the request fails (logged as a warning), the run still succeeds with deterministic gists. The payload reuses `summary`, `takeaways`, `deadline`, `noteTitle`, and `suggestSaveToNote`, so **Save to Notes & Archive** is unchanged.

### Configuration & Safety Contracts

- **Enable / Disable per assistant** plus global **Pause All / Resume All** in the **Agents** workspace.
- **Per-assistant knobs:** minimum age in days (`1–365`), max threads per run (`1–30`), include/exclude starred conversations, optional account scope, and cadence (`Manual Only`, `Daily`, `Weekly`).
- **Two-stage pipeline:** deterministic local SQLite candidate filtering first (the only stage `sender_hygiene` has, and enough for `stale_reads_digest` without a provider), followed by bounded AI verification using `ai.fast_or_main_model()` for `stale_reads` (`AgentStaleReads`) and `aging_replies` (`AgentAgingReplies`), and `ai.main_model()` for `resolved_and_expired` (`AgentResolvedSweeper`).
- **Evidence & non-reproposal:** every AI proposal must cite real candidate IDs and message excerpts; dismissed proposals are never re-proposed for the same thread revision, and proposals automatically become `superseded` if the thread is archived, trashed, currently snoozed (`conversation_snoozes`), or receives a new message.
- **Surface everywhere:** manage and triage proposals in **Agents** (`7`), and act on a thread's active suggestion directly inside Mail's right-side context panel.

---

## Future Ideas to Revisit

- **Context-Anchored Smart Snooze (User Idea #4):**
  - Date-anchored snooze (*"Renew by Dec 15"* -> snooze until 2 days before).
  - Meeting-anchored snooze (upcoming meeting with sender -> snooze until morning of meeting or attach to meeting prep).
  - Reading-window snooze (batch non-urgent long reads to a chosen slot like Saturday morning).
  - **Anti-loop escalation:** track repeated snoozes in `conversation_snoozes`; after 2–3 snoozes on the same thread, stop suggesting snooze and suggest **Turn into a Task**, **Draft a polite decline**, or **Archive**.
- **Inbox → Task / Note Graduation ("Evict from Inbox"):** Spot aging non-reply emails that represent an actionable chore or reference doc (receipts, itineraries, how-tos) and propose **Create Task & Archive** or **Save to Note & Archive**.
- **Assisted "Inbox Amnesty" & Post-Meeting Deflation:** A guided 2-minute sweep in **Today** grouping mail older than *N* days by reason, plus post-meeting prompts (*"Did your 2pm with Sarah cover these 2 threads? Archive?"*).
