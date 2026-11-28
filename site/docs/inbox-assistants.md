---
title: Inbox Assistants
nav: Inbox Assistants
summary: Configure standing assistants in Agents to summarize unread newsletters, roll old unread reads into a weekly digest, sweep handled or expired emails, draft replies, and spot senders you keep ignoring, all for your review.
reviewed: next
---

## Open Inbox Assistants {#open-inbox-assistants}

Open **Agents** from the sidebar or press <kbd>7</kbd> to manage standing **Inbox Assistants**. Assistants inspect aging mail in your Inbox and prepare suggestions for your review—nothing is ever archived, saved, or sent automatically.

<ts-shot scene="inbox-assistants" alt="Inbox Assistants showing configurable rules and pending suggestions awaiting review."></ts-shot>

- **AI provider:** Summaries, cross-thread checks, and reply drafts use the provider and model configured in **Settings → AI Provider**. Completed-task checks in **Expired & Already-Handled Sweeper**, all of **Sender Hygiene Advisor**, and **Stale Reads Digest** (which then shows each email's preview text instead of a written gist) run locally even without an AI provider.
- **Account scope:** Assistants follow the account selected in the sidebar or each assistant's configured **Mail Account Scope**.

## The Five Assistants {#the-three-assistants}

- **Stale Reads Summarizer** *(default: older than 7 days, up to 15 emails per run, excludes starred)*: Summarizes aging newsletters and long unread or unreplied messages with key takeaways and deadlines. Actions on each suggestion: **Archive**, **Save to Notes & Archive** (shown when the email looks worth keeping in **Notes**), **Create Task**, **Open Conversation**, or **Dismiss**.
- **Expired & Already-Handled Sweeper** *(default: older than 7 days, up to 15 emails per run, excludes starred)*: Flags aging emails whose dates or promotions have passed, whose sender you already handled in another thread or past meeting, or whose linked task you marked done. Actions include **Archive**, **View Resolving Thread**, **Reopen Task**, **View Completed Task**, **Create Task**, and one-click **Archive All N Safe** for expired emails or user-completed tasks.
- **Aging Reply Drafter** *(default: older than 3 days, up to 10 emails per run, includes starred)*: Prepares a draft reply for unanswered conversations from real people (excluding bulk mail and threads that already have a draft), plus a **Quick Check-In** option when a thread is 5 or more days old. Actions: **Review Draft** (opens the composer for editing), **Quick Check-In**, or **Archive**.
- **Sender Hygiene Advisor** *(default: unread for more than 14 days, up to 10 senders per run, excludes starred)*: Spots senders you keep ignoring and suggests dealing with the sender instead of one email at a time. A sender is flagged when you have dismissed at least three of their emails within a second of opening them (at least 60% of the ones you have dismissed in the last 90 days), or when three or more of their unread emails have sat in your Inbox past the age setting. It skips senders you have written to or replied to, mail you moved back to the Inbox, senders already muted or blocked, and senders whose suggestion you dismissed in the last 30 days. Each suggestion states the numbers, such as "Dismissed 6 of 7 emails within a second; 4 unread emails older than 14 days." Actions: **Unsubscribe** (when an email offers it; opens the usual confirmation), **Mute…**, **New Split…** (or *Sorted into* the split that already takes the sender), **Archive N in Inbox**, **Open Conversation**, or **Dismiss**.
- **Stale Reads Digest** *(default: unread for more than 7 days, up to 30 emails per run, excludes starred, runs weekly once enabled)*: Rolls unread newsletters, updates, and promotions older than the age setting into one card headed "Stale Reads Digest · N unread reads", oldest first, instead of a card per email. It skips conversations you have written in and anything the **Stale Reads Summarizer** already suggested. Each row shows the sender, subject, age, and a one-sentence gist (plus a deadline when one is stated). Actions: **Archive All N** (undoable), **Dismiss All**, and on each row **Save to Notes & Archive**, **Open Conversation**, or **Dismiss**. With an AI provider the gists are written by the model; without one, each row shows the email's preview text.

## Configure and Schedule Assistants {#configure-and-schedule}

Each assistant card in **Agents** can be toggled on or off independently:

- **Configure…** sets the minimum **Age** (1–365 days), **Max Threads per Run** (1–30), **Mail Account Scope**, **Run Schedule** (**Manual Only**, **Daily**, or **Weekly**), and **Include Starred Conversations**.
- **Run Now** runs one assistant immediately; **Run Enabled** runs every enabled assistant.
- **Pause All** and **Resume All** pause or resume background runs across all five assistants without changing their individual settings.

## Review Suggestions in Mail and Agents {#review-suggestions}

Suggestions appear in **Agents** (<kbd>7</kbd>) and in Mail's right-side context panel under **Inbox Assistant** whenever you open a matching conversation.

- In **Agents**, use <kbd>j</kbd> / <kbd>k</kbd> or <kbd>↓</kbd> / <kbd>↑</kbd> to move through suggestions, <kbd>o</kbd> or <kbd>Enter</kbd> to open the conversation, <kbd>e</kbd> to **Archive** the conversation, <kbd>n</kbd> to **Save to Notes & Archive**, <kbd>r</kbd> to **Review Draft**, and <kbd>x</kbd> to **Dismiss**. The Stale Reads Digest is a single stop: <kbd>e</kbd> archives all of its emails (undoable), <kbd>x</kbd> dismisses them all, and <kbd>o</kbd> or <kbd>Enter</kbd> opens the first.
- Suggestions clear automatically if you archive, trash, or snooze the conversation, or if a new message arrives. Dismissed suggestions are not shown again unless new mail arrives in that conversation.
- Suggestions you leave unanswered expire on their own: **Suggested Reply** after 7 days, **Stale Read** and **Stale Reads Digest** items after 14 days, **Expired & Already-Handled Sweeper** suggestions after 21 days, and **Ignored Sender** suggestions after 30 days. An expired suggestion is treated like a dismissed one, so it is not suggested again unless new mail arrives in that conversation.

## Privacy and Local State {#privacy-and-local-state}

- **Sender Hygiene Advisor** reads only your local mail and triage history and sends nothing to your AI provider. **Stale Reads Digest** sends nothing unless an AI provider is configured; with one, it sends the text of up to the configured number of emails (trimmed to about 1,500 characters each) in batched requests of at most 15.
- When the other assistants run manually or on a **Daily** / **Weekly** schedule, candidate email content—along with related threads and calendar events for **Expired & Already-Handled Sweeper**—is sent to your configured AI provider. See [What Is Shared](ai.html#what-is-shared).
- Every AI run is logged under **Settings → AI Provider → Activity** and obeys your **Request Controls** limits.
- Assistant configurations and pending suggestions live locally on this computer; they are excluded from encrypted cross-device sync and settings exports. See [Reading and Triage](triage.html) for other inbox cleanup tools.
