---
title: Reading and Triage
nav: Reading and Triage
summary: Work through your inbox from the keyboard — archive what's done, label and filter the rest, and split high-volume mail into its own tabs.
---

## The Triage Loop

Single-letter shortcuts work whenever you're reading and no text field has focus.

1. Move through conversations with <kbd>j</kbd> and <kbd>k</kbd>.
2. Press <kbd>e</kbd> to archive a conversation once it's done.
3. Reply with <kbd>r</kbd>, reply all with <kbd>a</kbd>, or forward with <kbd>f</kbd>.
4. Turn anything that needs follow-up into a task with <kbd>d</kbd> (see [Tasks and Goals](tasks-and-goals.html)).

Archiving shows a *Conversation archived* notice with **Undo**. Press <kbd>Shift + e</kbd> (**Mark Not Done**) to bring a conversation back to the inbox.

## Everyday Actions

| Action | Shortcut |
| --- | --- |
| Archive | <kbd>e</kbd> |
| Mark Not Done | <kbd>Shift + e</kbd> |
| Trash | <kbd>#</kbd> |
| Mark Spam | <kbd>!</kbd> |
| Toggle Read | <kbd>u</kbd> |
| Toggle Star | <kbd>s</kbd> |
| Manage Labels | <kbd>l</kbd> |
| Undo Last Action | <kbd>z</kbd> |

The [keyboard shortcut reference](keyboard-shortcuts.html) lists everything else.

## Batch Actions

Press <kbd>x</kbd> to select the current conversation, or use the checkboxes. A toolbar shows how many are selected and offers Archive, Trash, Mark Spam, Mark Read or Unread, Star, and Labels for all of them at once.

## Filters

Open **Filters** above the list to narrow the current mailbox. Each filter has a shortcut:

- **Unread** — <kbd>Shift + u</kbd>
- **Starred** — <kbd>Shift + s</kbd>
- **Important** — <kbd>Shift + i</kbd>
- **No Reply** — <kbd>Shift + r</kbd>, conversations you haven't replied to yet

## Split Inboxes

<ts-shot scene="split-inbox" alt="An inbox with Split Inbox tabs for newsletters and a client."></ts-shot>

Split Inboxes move high-volume mail into its own tab, so the main inbox stays focused on people.

1. Click the **Add Split** icon beside the conversation count, or open **Settings → Split Inboxes**.
2. Give it a name, choose the account, and pick **Match By**: **Sending Domain**, **Label**, or **Address Contains**.
3. Click **Add Split Inbox**.

Press <kbd>Tab</kbd> and <kbd>Shift + Tab</kbd> to move between tabs. Each split belongs to one account, so tabs appear when that account is in focus rather than in the combined view of all accounts. Deleting a split never deletes mail.

## Search

Press <kbd>/</kbd> to search. Search looks through all your mail, including archived conversations, and shows results from this computer right away. When you stop typing, it also asks the server for older mail. Results still in your inbox are marked **Inbox**. Click **Inbox Only** to search just your inbox. Trash and Spam are left out unless you add `in:trash`, `in:spam`, or `in:anywhere`.

You can narrow a search with `from:`, `to:`, `subject:`, `before:` and `after:` (YYYY/MM/DD), `is:unread`, `is:starred`, `label:`, `in:`, and `has:attachment`. `to:` also finds mail where the person was on Cc or Bcc. Put a minus in front of a term to leave it out (`invoice -from:billing@example.com`), and use `OR` or braces for either-or (`from:ana OR from:bo`, or `{from:ana from:bo}`). As you type, suggestions appear below the search box: operator names, people from your contacts after `from:` or `to:` (choosing someone with more than one address searches all of them), your labels after `label:` or `in:`, and dates like **Past month** after `after:` or `before:`. Use the arrow keys and <kbd>Enter</kbd>, or click, to choose one. <kbd>Escape</kbd> closes the suggestions, and a second <kbd>Escape</kbd> clears the search. Click the empty search box to see your recent searches, which stay on this computer.

## Moving Around

- <kbd>0</kbd> opens Today, and <kbd>1</kbd>–<kbd>5</kbd> switch between Mail, Calendar, Tasks, Contacts, and Files.
- <kbd>g</kbd> then <kbd>i</kbd>, <kbd>a</kbd>, <kbd>t</kbd>, <kbd>d</kbd>, <kbd>o</kbd>, or <kbd>s</kbd> goes to Inbox, All Mail, Trash, Drafts, Outbox, or Sent.
- <kbd>Escape</kbd> goes back after you jump from one conversation to another, for example from a task or a contact.

## Today

Press <kbd>0</kbd> for your day at a glance. Today lists your agenda when a calendar is connected, with finished and declined events dimmed, a **Now** line before what is still ahead, the first few events **Tomorrow**, and a **Join** button for your next online meeting. Unread mail is sorted into **Invitations** (meeting requests to answer), **Needs a reply** (written to you and at most two others by someone you write to), **From people**, and **Updates** (newsletters, notifications, and no-reply senders). **Open Inbox**, **Open Calendar**, and **Open Tasks** at the foot of each column go to the full view. Tasks that are overdue or due today come first, followed by your other open tasks and people due for a **Keep in Touch** check-in. Mail and tasks follow the account you have selected; the agenda always includes every calendar you show in **Calendar**. To open the app on Today, choose **Today** under **Settings → Appearance → Start With**.

<ts-shot scene="today" alt="Today with the day's agenda and tomorrow's first events, unread mail grouped by Needs a reply, From people, and Updates, and tasks due beside Keep in Touch reminders."></ts-shot>

## Inbox Zero

An empty inbox shows **Inbox zero** and when that account was last checked, or whether it needs reconnecting or could not be checked. The side panel links to **Today** for your agenda, tasks, and Keep in Touch reminders. If filters hid every conversation, use **Clear Filters** to bring them back.

<ts-shot scene="inbox-zero" alt="An empty inbox with last-check status and a side panel linking to Today."></ts-shot>

## Newsletter Controls

Newsletters and bulk mail show the sender’s **Recent emails** beside the message. Use **Unsubscribe**, **Archive N in Inbox**, **Mute…**, or **New Split…** below the list. Archive clears that sender’s existing inbox conversations, with **Undo**. Mute marks new mail read and archives it as it arrives; it can also archive existing inbox mail. Manage or unmute senders in **Settings → Muted Senders**. Muted senders are shared through encrypted sync when enabled; update all your devices to use them.

<ts-shot scene="newsletter" alt="A newsletter with its sender’s recent emails and unsubscribe, archive, mute, and split controls."></ts-shot>

## Unsubscribe

When a message supports it, press <kbd>⌘/Ctrl + u</kbd> or use the reader's unsubscribe button. ThreeStrands asks before it sends a one-click request, opens an unsubscribe email, or opens the sender's page.

## Trust and Attachment Warnings

ThreeStrands checks each message for common email tricks, entirely on your computer, and shows what it finds beside the sender's name:

- A small gray shield marks a **Known sender**: someone you write to or have saved as a contact, whose email is verified as coming from them. It is not a promise that every message is safe.
- An amber warning names what looks wrong, such as a familiar name used by a new address, a lookalike domain, a subject line that disguises words with look-alike letters, a misleading link, or a risky attachment from someone you have not written to.

Hover over the shield or warning to see the sender details, or click it to keep them open. The most serious warnings (a borrowed name, a lookalike address or link, or a risky attachment) also appear above the message itself.

When something looks wrong, the side panel shows a **Be careful** section with every finding. For newsletters and other bulk mail, a **Sender checks** section shows the same details even when nothing looks wrong.

Choose **Show link** to find the link in the message. **Details** expands the mail server’s sender checks, the sites links open, and tracking images. Links in folded quoted history are left out of these warnings. Remote images remain blocked until you allow them, subject to your privacy settings.

<ts-shot scene="trust" alt="The Be careful section explains a misleading link and offers a Show link action."></ts-shot>

Risky attachments, including programs, macro-enabled Office files, disk images, attached web pages, and disguised filenames such as invoice.pdf.exe, show a warning and reason before opening. They are never previewed.

## Files

Press <kbd>5</kbd> to see attachments from your synced mail in one place. Search by filename, filter by sender, type, direction, account, and date, and star the files you come back to. Stars stay on this computer and do not keep an offline copy. On macOS, a selected safe file can show a preview above its details; click the preview to open it. **Mark Spam** removes that email’s files from Files, and **Undo** restores them.
