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
   - *Action:* Summarizes the core takeaways with cited message evidence and offers **Archive** or **Save to Notes & Archive** (evicting the email from Inbox while keeping the summary searchable in Notes).
2. **Expired & Already-Handled Sweeper (`resolved_and_expired`)**
   - *Target:* Aging Inbox threads that are either already resolved or past their useful window:
     - Deterministic zero-token check: a linked ThreeStrands task on the thread was already completed after the email arrived.
     - Cross-entity AI verification: a newer thread with the same external participant where the user replied, a calendar event with the sender that already occurred, or an explicit deadline/event/promo date that has already passed.
   - *Action:* Proposes **Archive** with clickable resolving evidence (thread, completed task, or expired date) and Undo.
3. **Aging Reply Drafter (`aging_replies`)**
   - *Target:* Aging Inbox threads where the latest message is from an external human sender (excluding bulk/automated mail and threads that already have a draft).
   - *Action:* Drafts a natural reply for the user's review—plus an optional low-friction check-in angle when the thread is 5+ days old—and opens the ThreeStrands composer pre-filled for editing. Never auto-sends.

### Configuration & Safety Contracts

- **Enable / Disable per assistant** plus global **Pause All / Resume All** in the **Agents** workspace.
- **Per-assistant knobs:** minimum age in days (`1–365`), max threads per run (`1–30`), include/exclude starred conversations, optional account scope, and cadence (`Manual only`, `Daily`, `Weekly`).
- **Two-stage pipeline:** deterministic local SQLite candidate filtering first, followed by bounded AI verification using the existing fast/reasoning model tiers (`summary` / `replyDraft` vs. `resolved_and_expired`).
- **Evidence & non-reproposal:** every AI proposal must cite real candidate IDs and message excerpts; dismissed proposals are never re-proposed for the same thread revision, and proposals automatically become `superseded` if the thread is archived, trashed, or receives a new message.
- **Surface everywhere:** manage and triage proposals in **Agents** (`7`), and act on a thread's active suggestion directly inside Mail's right-side context panel.

---

## Future Ideas to Revisit

- **Context-Anchored Smart Snooze (User Idea #4):**
  - Date-anchored snooze (*"Renew by Dec 15"* -> snooze until 2 days before).
  - Meeting-anchored snooze (upcoming meeting with sender -> snooze until morning of meeting or attach to meeting prep).
  - Reading-window snooze (batch non-urgent long reads to a chosen slot like Saturday morning).
  - **Anti-loop escalation:** track repeated snoozes in `conversation_snoozes`; after 2–3 snoozes on the same thread, stop suggesting snooze and suggest **Turn into a Task**, **Draft a polite decline**, or **Archive**.
- **Batched Stale Reads Digest:** Weekly roll-up of all unread newsletters/articles older than 7 days into one scannable card with one-click **Archive all N** and per-item **Save to Notes**.
- **Inbox → Task / Note Graduation ("Evict from Inbox"):** Spot aging non-reply emails that represent an actionable chore or reference doc (receipts, itineraries, how-tos) and propose **Create Task & Archive** or **Save to Note & Archive**.
- **Behavioral Sender Hygiene:** Combine aging unread counts with `src/triage.ts` quick-dismissal telemetry (`isQuickTriageDisposition` < 1s) to suggest **Unsubscribe**, **Mute**, or **Create a Split** when a sender is repeatedly ignored or dismissed.
- **Assisted "Inbox Amnesty" & Post-Meeting Deflation:** A guided 2-minute sweep in **Today** (during Plan Today / Shutdown) grouping mail older than *N* days by reason, plus post-meeting prompts (*"Did your 2pm with Sarah cover these 2 threads? Archive?"*).
