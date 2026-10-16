# Changelog

What's new in ThreeStrands, written for the people who use it. Each release's notes on GitHub and the changelog page on the website come from this file.

## Unreleased

### New
- In **Settings → Signatures**, **Import from Gmail…** brings in a signature from a connected Gmail account for preview, naming, and optional use as the account’s default for new messages. You can edit its plain-text alternative separately or regenerate it from the visual signature.
- The **Trust** section in the side panel now spots common email tricks using only what's on your computer: a familiar name sent from a new address, an address or link that imitates a domain you write to, links that hide where they go, and risky attachments from people you haven't written to. It says **Be careful** when something looks wrong, shows **Known sender** for people you write to, and **Show link** scrolls to the link in question. Sender checks, linked sites, and tracking images are under **Details**. **Known sender** also appears when a forwarding or filtering service left out the usual sender check, as long as the email is confirmed to come from the sender's own domain.
- When your inbox is empty, the side panel now shows what's next: your open tasks, people you're due to reconnect with from **Keep in Touch**, and today's agenda when you have a calendar connected. Click any item to open it.

### Improved
- The side panel now shows which person its history is about, with one name at the top of that section and a **2 others** button to switch to anyone else on the conversation. Your own colleagues are listed last. As you scroll or move between messages, the panel follows whoever wrote the message you're reading.
- **Find Times** and **Get Brief** now sit together as one row of buttons at the top of the side panel, and each opens below when it has something to show. **Find Times** works while you're reading a conversation, too: pick the times you want and **Draft Reply With These Times** starts your reply.
- **Upcoming meetings** in the side panel show short dates like **Today**, **Tomorrow**, or **Fri**, so long meeting names wrap onto a second line instead of being cut off. Hover a date for the full day.
- The side panel no longer lists other people at the same company. Grouping by email domain mixed in unrelated people at large companies and shared mail providers.
- **Draft Review** now opens below your message from **Review** in reply and new-message windows. In replies, **Draft Reply** and **Review** sit beside the quoted-conversation toggle, with a single divider above **Send**.
- Replies now hide the signature picker when no signature is applied. Your signature’s hosted images appear automatically while composing, without image removal or resize controls over the logo.
- **Settings → Signatures** now uses a visual editor with a compact formatting toolbar, font and color pickers, and image editing when you click a logo. **Import…** is under **More**, and your imported layouts, hosted images, and separate plain-text alternative are preserved.
- Messages sent to a long list of people now show the first 20 recipients followed by "and N more". Click it to see everyone.
- The **Task details** heading in the **Tasks** side panel and the **Brief** heading in the conversation side panel now have icons, matching the other side-panel headings. AI buttons now show their sparkles in the same color everywhere.
- Reply and new-message windows give more room to writing, with a compact signature picker below the message and **Send later…** in the **Send** menu. Replies also offer **Send & archive** in that menu and a shorter **Draft Reply** button on the right; keyboard hints now match your platform.
- When your inbox is empty, the reading pane now says **Inbox zero** and shows when that account was last checked, or that it needs reconnecting or couldn't be checked, so an empty inbox is never mistaken for a stalled sync. Problems with your other accounts no longer show up there. The inbox header is tidier too: **Main** appears only once you have a split inbox, and Add Split and **Filters** are now compact icon buttons beside the conversation count. When filters hide every conversation, **Clear Filters** brings them back.
- Pasting or importing a signature now keeps hosted HTTPS image URLs by default, including images too large to embed. **Embed Remote Images** is tucked under **Image Options** in the Gmail import dialog when you want to send small images with the message instead.

### Fixed
- The **Trust** section in the side panel no longer warns about links in the quoted earlier emails that are folded away in a reply. Those links are often rewritten by someone else's mail system, so their text and destination differ without anything being wrong.
- In **Settings → Signatures**, dialogs now scroll so you can reach the plain-text field and import controls, previews show logos on a white default background, and **Load Images** lets you see hosted logos while editing. Editing plain text during Gmail import now responds without reprocessing the whole signature, formatted phone numbers are no longer repeated, and the **Choose File** button and its label are centered in the file field. Divider lines in a signature, such as a vertical line beside a logo or a rule above a tagline, now stay in the preview and in the messages you send.

## 0.133.2 — 2026-10-15

### New
- Create reusable email signatures with formatted text, links, and logos in **Settings → Signatures**, and choose defaults for each sending address. Switch or remove signatures while composing, with separate defaults for new messages and replies or forwards.
- A **Sent** folder lists the conversations you've sent messages in, including ones sent from your other devices or from webmail. Choose it from the folder menu or press G then S.
- In **Contacts → Groups**, **Email Group** starts a new message addressed to everyone in the group.
- **Edit as New** starts a fresh message from one you sent before, with its subject, text, and attachments but no recipients, so you can send the same note to new people. Use the button beside **Forward** on any message you sent, or press Shift+C. It works in the new **Sent** folder too.

### Improved
- The **Outbox** now holds only mail that hasn't finished sending: messages you can still undo, scheduled sends, and ones that failed or need a check. Once a message goes out it moves to **Sent**. The Outbox also works like your other folders now: choose a message on the left to read it on the right, with its delivery status and actions such as **Undo Send** and **Reschedule** above it.
- For newsletters and other bulk mail, the side panel now lists the sender's earlier emails in **Recent emails** instead of a separate card, with **Unsubscribe**, **Mute…**, and **New Split…** as small links at the bottom so they don't compete with the email you're reading.
- Buttons that sit side by side, such as **Add Members** and the delete button on a contact group, now line up with each other.
- When adding members to a group, the people you've chosen now appear inside the search field, the search clears after each pick, and Backspace removes the last person.
- A message you **Forward** now stays in the conversation it came from, so you can see what you sent on and read your coworkers' replies alongside the original thread.
- Finding times to meet now happens right in the side panel. With **Draft Assist** turned on in **Settings → AI Provider**, **Find Times** reads the conversation and your draft, so when someone asks to connect next week you get times next week, with a line saying why. Type a change such as "Thursday or Friday afternoon" and click **Adjust**, or narrow times to **Mornings** or **Afternoons**, move to **Later** dates, or click **Show More**, all without a second calendar column squeezing the email you're reading. If a brief already suggested a meeting, Find Times starts from the dates it found. Asking "When am I free Thursday afternoon?" in **Ask About This Conversation** now shows only Thursday afternoons. The **Find a time** section of the day calendar has been retired in favor of this.
- In **Files**, the **File details** heading now has a file icon that lines up with the other icons in the side panel.
- Buttons that send something to your AI provider now all carry the sparkle icon, so you can tell at a glance when AI is involved. AI features you switch off no longer leave reminders in the side panels or the command palette; until you choose an AI provider, the **Brief** section offers a single **Set Up AI** link. The command palette now calls the brief **Get Brief**, as the button does.

### Fixed
- Tables pasted from a spreadsheet, such as a price list, no longer show up as a tall blank box with nothing inside.
- Adding members to a group no longer offers to create a new contact for an address that already belongs to someone in your address book; it picks that person instead. Typing the address of someone already in the group now says so rather than adding them again.
- The folder menu (Inbox, Drafts, Outbox, and the rest) works again while a draft is open, and switching folders saves what you've typed first.
- In **Calendar**, the button that hides the side panel no longer covers the event count next to **Today's agenda**.
- In **Files**, the sender in **File details** no longer has "Opens in Contacts" stuck to the end of their name.
- **Draft Reply With AI** and **Enhance With AI** now say what setup is missing when no API key is configured, instead of doing nothing or failing after you click.

## 0.125.4 — 2026-10-15

### Improved
- Removing a Gmail account or Google Calendar now also signs ThreeStrands out at Google, so the access it had stops working instead of staying active until you revoke it yourself. If ThreeStrands can't reach Google, the account stays put so you can try again. Removing only the mail or only the calendar keeps you signed in while the other is still connected on this computer, and if you sync between devices, **Disconnect this device** leaves your other devices signed in.

### Fixed
- Buttons in some emails, such as security alerts, no longer lose their padding and show their label squeezed against the top edge.
- Some emails no longer show small stray gaps between images, buttons, or blocks the sender placed tightly together.
- In **Settings → Mail Accounts**, the Gmail or IMAP badge under **On Your Other Devices** now lines up with the **Set Up Here** button.

## 0.125.1 — 2026-10-14

### New
- You can now set more than one block of working hours on a day in **Settings → Availability**, for example 9:00–12:00 and 13:00–17:00 to keep your lunch hour free. Suggested meeting times leave the gaps alone.
- Image attachments on a message now show a small preview of the picture, so you can tell them apart without opening each one.
- When an email announces events, such as a school or club calendar, **Get Brief** and **Ask about this conversation** now offer each one as an event you can add to your calendar at its stated time. Events with a date but no time yet are added as all-day events.
- New events can now be marked **All day** and given a location.
- Newsletters and other bulk mail now show who sent them in the side panel: how many emails they've sent, how often, and how many you've read, with one-click **Unsubscribe**, **Archive** for everything still in your Inbox from them, and **New Split…** to give them their own tab. **Mute…** goes further: new mail from that sender is marked read and archived as it arrives, on all your devices, and you can review muted senders in **Settings → Muted Senders**. If you sync between devices, update ThreeStrands on each of them to share muted senders.
- A new **Trust** section shows whether your mail server verified the sender, which sites the links open, and any tracking images. It also appears on any email where a link's text names a different site than it opens, or replies would go to another organization.
- Attachments that could harm your computer, such as programs, Office files with macros, disk images, attached web pages, and files disguised as documents like invoice.pdf.exe, now show a warning and the reason in the message and in **Files**, before you open them.
- **Files** can now mark a file's email as spam with **Mark Spam**, which removes its files from **Files**. **Undo** brings them back.
- On macOS, **Files** now shows a preview of the selected file, such as the first page of a PDF or document, above its details. Click the preview to open the file. Files that could harm your computer are never previewed.

### Improved
- Times you offer or confirm in a reply now end with a familiar time zone abbreviation such as EDT or PST, instead of a name like America/New_York.
- Attachments on a message now say what each part does: click the filename to open it, use the save button to save a copy, and the star at the end keeps it in **Files**. Attachments stacked in a column now line up.
- Suggestions now read up to 10,000 characters of each message, so events further down a long newsletter aren't missed.
- In **Files**, the details panel now lines up with its heading, and the **Star** button has a label like **Open** and **Save as…** beside it. Hover over the sender's name to see their contact card, or click it to open them in **Contacts**. **Open** is now the main button, and **Source conversation** shows just the email's subject instead of repeating the sender and date.

### Fixed
- Returning to **Files** no longer briefly shows files from emails you marked as spam or deleted while you were away.
- Emails written as preformatted text, such as some security and system notices, now wrap long lines to fit the reading pane instead of cutting them off at the edge.
- Small icons in emails that specify only an image height now keep their proportions instead of appearing squeezed beside text.
- Hovering over a message no longer shows a stray "Message content" tooltip.
- An email that lists more than 10 events or to-dos no longer makes suggestions fail. The first 10 are shown, along with a note saying how many more there were.
- Suggested meeting cards no longer flash "Checking your calendar…" from time to time while you read.
- Adding an announced event to your calendar no longer invites the person who sent the email.
- The notice that an account is set up on another device is now a small message in the bottom-left corner, alongside app update notices, instead of a panel filling the window.
- In the side panel's **Files** list, the star no longer covers the date. It now sits after the show-email button, under the date.
- The reply, reply all, and forward buttons beside a sender's name now line up with the name instead of sitting slightly below it.

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
