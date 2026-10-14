# Changelog

What's new in ThreeStrands, written for the people who use it. Each release's notes on GitHub and the changelog page on the website come from this file.

## Unreleased

## 0.116.1 — 2026-10-14

### New
- Accounts you set up on another computer now appear in **Settings → Mail Accounts** under **On Your Other Devices**, instead of showing as broken. Choose **Set Up Here** for the ones you want on this computer, so a personal account doesn't have to follow you to a work laptop.
- Importing an encrypted settings file lets you choose which of its mail accounts to set up on this computer.

### Fixed
- Background images in messages, such as social icons and banner panels, now keep the size and position the sender gave them instead of appearing cropped.
- After installing an update on macOS, ThreeStrands now reopens in front of your other windows instead of behind them.

## Highlights: September to October 2026

ThreeStrands went from its first line of code to its first public releases in about a month. These are the things it can do today.

### New
- **Gmail and IMAP in one inbox.** Connect several Gmail accounts and any IMAP/SMTP server, including Proton Mail through Proton Mail Bridge. Read them together in one inbox or focus on one account at a time.
- **Keyboard-first triage.** Archive, star, label, and undo without leaving the keyboard. Split Inboxes divide your mail into tabs, and the command palette (⌘/Ctrl + K) finds every action by name.
- **Search that understands you.** Narrow searches with `from:`, `to:`, `subject:`, dates, labels, and attachments across every account. **Search All Mail** also asks the server for older mail.
- **Writing tools.** Rich text, snippets, attachments, Send Later, a 10-second undo window, and checks before a message goes out.
- **Tasks and goals.** Turn a conversation into a follow-up and roll your follow-ups up to goals for each account.
- **Calendar.** Connect Google Calendar, plan your week in Day, 3-day, Workweek, or Week view, create events, and share your free times. On macOS, ThreeStrands can open email links and `.ics` invitations.
- **Contacts.** Profiles, groups you can address in one step, birthdays, Keep in Touch reminders, and CSV and vCard import and export.
- **Files.** Every attachment from your mail in one place, with filters for sender, date, type, and account. Star the ones you'll need again.
- **Optional AI assistance.** Bring your own provider key for conversation summaries, thread chat, suggested tasks and meetings, and reply drafts. Nothing is sent until you turn it on.
- **Settings transfer and sync.** Move your setup to a new computer with an encrypted export, or keep several computers in step with end-to-end encrypted sync.
- **A Getting Started checklist** that checks itself off as you connect mail and try the essentials.
- **One-step unsubscribe** from mailing lists that support it, with a confirmation first.

### Improved
- **Mail looks the way the sender designed it.** Messages render faithfully inside a locked-down sandbox, remote images stay behind a proxy until you allow them, and long quoted replies fold out of the way.
- **A calmer, more consistent interface.** Buttons, type, spacing, and icons now follow one design system across mail, tasks, calendar, and settings.
