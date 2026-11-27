---
title: Inbox Assistants
nav: Inbox Assistants
summary: Configure standing assistants in Agents to summarize unread newsletters, sweep handled or expired emails, and draft replies for your review.
reviewed: next
---

## Open Inbox Assistants {#open-inbox-assistants}

Open **Agents** from the sidebar or press <kbd>7</kbd> to manage standing **Inbox Assistants**. Assistants inspect aging mail in your Inbox and prepare suggestions for your review—nothing is ever archived, saved, or sent automatically.

- **AI provider:** Summaries, cross-thread checks, and reply drafts use the provider and model configured in **Settings → AI Provider**. Completed-task checks in **Expired & Already-Handled Sweeper** run locally even without an AI provider.
- **Account scope:** Assistants follow the account selected in the sidebar or each assistant's configured **Mail Account Scope**.

## The Three Assistants {#the-three-assistants}

- **Stale Reads Summarizer** *(default: older than 7 days, up to 15 emails per run, excludes starred)*: Summarizes aging newsletters and long unread or unreplied messages with key takeaways and deadlines. Actions on each suggestion: **Archive**, **Save to Notes & Archive** (shown when the email looks worth keeping in **Notes**), **Create Task**, **Open Conversation**, or **Dismiss**.
- **Expired & Already-Handled Sweeper** *(default: older than 7 days, up to 15 emails per run, excludes starred)*: Flags aging emails whose dates or promotions have passed, whose sender you already handled in another thread or past meeting, or whose linked task you marked done. Actions include **Archive**, **View Resolving Thread**, **Reopen Task**, **View Completed Task**, **Create Task**, and one-click **Archive All N Safe** for expired emails or user-completed tasks.
- **Aging Reply Drafter** *(default: older than 3 days, up to 10 emails per run, includes starred)*: Prepares a draft reply for unanswered conversations from real people (excluding bulk mail and threads that already have a draft), plus a **Quick Check-In** option when a thread is 5 or more days old. Actions: **Review Draft** (opens the composer for editing), **Quick Check-In**, or **Archive**.

## Configure and Schedule Assistants {#configure-and-schedule}

Each assistant card in **Agents** can be toggled on or off independently:

- **Configure…** sets the minimum **Age** (1–365 days), **Max Threads per Run** (1–30), **Mail Account Scope**, **Run Schedule** (**Manual Only**, **Daily**, or **Weekly**), and **Include Starred Conversations**.
- **Run Now** runs one assistant immediately; **Run Enabled** runs every enabled assistant.
- **Pause All** and **Resume All** pause or resume background runs across all three assistants without changing their individual settings.

## Review Suggestions in Mail and Agents {#review-suggestions}

Suggestions appear in **Agents** (<kbd>7</kbd>) and in Mail's right-side context panel under **Inbox Assistant** whenever you open a matching conversation.

- In **Agents**, use <kbd>j</kbd> / <kbd>k</kbd> or <kbd>↓</kbd> / <kbd>↑</kbd> to move through suggestions, <kbd>o</kbd> or <kbd>Enter</kbd> to open the conversation, <kbd>e</kbd> to **Archive**, <kbd>n</kbd> to **Save to Notes & Archive**, <kbd>r</kbd> to **Review Draft**, and <kbd>x</kbd> to **Dismiss**.
- Suggestions clear automatically if you archive, trash, or snooze the conversation, or if a new message arrives. Dismissed suggestions are not shown again unless new mail arrives in that conversation.

## Privacy and Local State {#privacy-and-local-state}

- When an assistant runs manually or on a **Daily** / **Weekly** schedule, candidate email content—along with related threads and calendar events for **Expired & Already-Handled Sweeper**—is sent to your configured AI provider. See [What Is Shared](ai.html#what-is-shared).
- Every AI run is logged under **Settings → AI Provider → Activity** and obeys your **Request Controls** limits.
- Assistant configurations and pending suggestions live locally on this computer; they are excluded from encrypted cross-device sync and settings exports. See [Reading and Triage](triage.html) for other inbox cleanup tools.
