# Changelog

What's new in ThreeStrands, written for the people who use it. Each release's notes on GitHub and the changelog page on the website come from this file.

## Unreleased

### Fixed
- Meeting suggestions now prefer the full conferencing join link over a generic location such as “Microsoft Teams.” Emails offering multiple sessions keep each session paired with its own join link.

## 0.163.2 — 2026-10-24

### New
- **Settings → Appearance → Email Zoom** now enlarges email text, images, and spacing together, independently of app text size. The reading controls explain how to combine zoom with a minimum font size, or preserve the sender’s proportions by leaving the minimum off.
- The new **Today** view (press <kbd>0</kbd>) shows your day at a glance: your agenda with a **Join** button for the next meeting, unread mail sorted into **Invitations**, **Needs a reply**, **From people**, and **Updates**, and tasks that are overdue or due today. Finished events are dimmed under a **Now** line, and the first few events **Tomorrow** are listed below. The agenda includes all your calendars, while mail and tasks follow the account you have selected. Its lists use the same text size as your inbox, with room to breathe. To start the app there, choose **Today** under **Settings → Appearance → Start With**; when your inbox is empty, the side panel links to it.
- You can now edit the message you're forwarding. Click **Edit** on the bar above the forwarded message to trim it or add notes. Some formatting and web images may be simplified, and **Restore Original** brings back the message as you received it.
- Calendar invitations now offer separate **Accept**, **Maybe**, and **Decline** buttons, plus **Suggest a different time**, in the side panel, replacing duplicate AI event suggestions; choosing alternative times prepares a reply for you to review. Other events suggested in the **Brief** recognize when they’re already on your calendar, even at a different time or under a new name, with **Open in Calendar** to see them, **Dismiss** to clear the suggestion, and **Add Again** in **⋯** if you need another copy.
- The header of an open email now shows who was copied, as in "to David Martinez, cc me", so you can tell whether a message was sent to you or you were only Cc'd. Your own address shows as "me".
- Meeting details in the **Calendar** now show who has accepted or declined. The guest count adds a tally such as "2 going, 1 not going", and each guest is marked **Going**, **Not going**, **Maybe**, or **Awaiting**.
- ThreeStrands now reopens where you left off. If you quit while in **Today**, **Calendar**, **Tasks**, **Contacts**, or **Files**, that view is open the next time you start the app.
- **Settings → AI Provider** now lists recent **Activity**: which feature sent each request to your AI provider, the conversation's subject, how many messages, and the tokens used. Email text is never kept there.

### Improved
- Switching accounts with <kbd>⌘1</kbd>, <kbd>⌘2</kbd>, and so on now remembers the email you had open in each account, so it's still selected when you come back.
- Pressing <kbd>t</kbd> now switches the column on the right of your mail between the conversation's details and the day's **Schedule**, instead of squeezing in another column. Your choice stays put when you visit another view or restart the app.
- Calendar invitation cards now make web addresses in the location clickable, opening them in your browser, and keep decorative separator lines out of descriptions. In the side panel, unanswered invitations now say **Not responded.**, and conflicting meeting titles appear in quotation marks.
- The **Brief** now sums up what's new since your last reply or your last brief, instead of the whole conversation again, and suggests tasks only from the new messages. When it helps, a last line recaps the earlier conversation, and a note says when the brief covers only the latest messages.
- Meeting suggestions in the **Brief** are easier to act on. Ways to narrow the times (**Any Time**, **Mornings**, **Afternoons**) and the arrows for earlier or later days sit above the times, picked times show a checkmark, and **Draft Reply With These Times** stands out as the main button. **Edit** and **Dismiss** move to the card's **⋯** menu, and **Check details** now opens the editor.
- Meetings suggested in the **Brief** show where and who with small icons, and list names in reading order, such as "Joshua Dadeboe" rather than "Dadeboe, Joshua". Online meetings get a **Join** link when the email or your calendar has the meeting address. **Discard** on a suggestion is now **Dismiss**, and the email excerpt sits at the bottom of the card.
- The **Brief** section collapses from its heading like the rest of the side panel, and its bullets and text now line up with the section headings. It also says how many messages were shared with AI and with which provider.
- Discarding drafts, or failed messages in **Outbox**, now works like moving conversations to Trash: it happens right away, and **Undo** in the notice (or <kbd>z</kbd>) brings them back, attachments included, for up to an hour. Failed messages come back to **Drafts**.
- Collapsed emails in a conversation now use the same text sizes as the rest of the app, instead of larger type.
- An unsent draft at the end of a conversation now lines up with the emails above it, with **Resume Draft** on the right.
- You can now reorder accounts in **Settings → Calendar Accounts** with the up and down arrows, the same way as mail accounts. The **Calendar** sidebar follows the new order, and reconnecting an account no longer moves it to the bottom of the list.
- Files you attach to a message now appear right above **Send** and the paperclip button, so you can see them as soon as you add them instead of having to scroll down past your signature.
- When suggested meeting times can't be checked against one of your calendars, a single note under the times now names that calendar and links to **Calendar Settings**, instead of "not every calendar checked" on every time. Times for an email that doesn't give a time zone now say which zone they use in familiar terms, such as "Eastern Time".

### Fixed
- The agenda in **Calendar** no longer lists a meeting twice when it is on two of your calendars; it shows the copy you haven't declined. Declined meetings are dimmed, and calendars named after your account no longer repeat the address.
- **Settings → Appearance** text size, font, and minimum email font size now stay on each Mac instead of syncing to your other devices, so you can use larger text on a laptop without changing it on a big monitor.
- Emails that set their text size, color, or weight with older-style formatting now look the way the sender designed them, instead of showing headline figures as small bold text. With a minimum font size set in **Settings**, footnote numbers no longer push their line apart.
- Gmail drafts now stay in step for accounts with no drafts. Before, if you sent or deleted your last draft in Gmail or on another device, it could stay behind in **Drafts** here.
- A message you forward now shows in its conversation as soon as you click **Send**, the way replies do, instead of appearing only after it finishes sending.
- Clicking a conversation in the message list while writing a new message now opens it. Your message is saved to **Drafts** so you can finish it later.
- Newsletters that space out their sections with blank table rows, such as article cards with a gap between the headline, image, and summary, now keep that spacing instead of running the sections together.
- Full-size photos taken on recent phones now load in emails when remote images are on, instead of showing as empty boxes.
- Images in emails from senders whose image servers screen out non-browser visitors now load when remote images are on, instead of staying blank.
- The **Brief** for an email you're only copied on no longer treats requests made of someone else as yours, and doesn't suggest them as tasks for you. Briefs are also shorter, so a short email gets a single line.
- With **Drive Time** on, suggested meeting times no longer warn that a calendar wasn't checked when only its travel time couldn't be read. Its busy times are still checked.

## 0.151.3 — 2026-10-21

### Improved
- **Drafts** now looks like your other folders, showing who each draft is to, its subject, and a preview. You can select several drafts with ⌘-click, Shift-click, or ⌘A and discard them together, after confirming.
- In **Outbox**, you can now select several messages the same way: return the ones still waiting to send to **Drafts**, or discard the ones that failed. Its rows also line up with every other folder.
- Newsletters, receipts, and notifications now preview in the message list with the summary line their sender wrote for inbox previews, like "Your login code is 564368" or "Delivered 1 item", instead of a logo name or a "View in browser" link.

### Fixed
- Shift-clicking a checkbox in a folder list now selects every message between it and the last one you checked, as Shift-clicking a row already did.
- Emails that center their layout with older formatting, common in newsletters and promotions, now appear centered in the reading pane instead of pinned to the left edge.
- Some newsletters and promotions previewed in the message list as placeholder codes like `${Headline-Text}`, or as a note that your email software can't display them, instead of their words. The preview, and searches, now use the text the email actually shows.

## 0.150.1 — 2026-10-20

### New
- Drafts on Gmail accounts now save to your Gmail **Drafts**, so you can pick them up in Gmail on your phone or the web. ThreeStrands saves a few seconds after you stop typing, and right away when you close the message. The composer says **Draft saved** once Gmail has it, or tells you when it's waiting to reconnect. Sending or discarding the draft removes it from Gmail.
- Drafts you start or change in Gmail, on any device, now appear in **Drafts** within a minute, attachments included. A reply you started elsewhere shows at the end of its conversation with **Resume Draft**, and pressing **Reply** opens it. If a draft changes both here and in another app, the composer asks which version to keep.
- Drafts now reach your other computers through Gmail instead of **Replicated Sync**, so attached files come along. A signature added on another computer arrives as part of the message text.
- Drafts on IMAP accounts, including Proton Mail Bridge, now save to the account's Drafts mailbox and come back from it, just like Gmail drafts, so you can finish them in another mail app. If you didn't map a Drafts mailbox during setup, drafts stay on this computer.

### Fixed
- The composer now says **Draft saved** instead of "Saved on this device" once a draft is safely stored, including while it waits to sync or needs your attention, and the **Drafts** list no longer says "Saved on this device" either.
- After you reply to a conversation, your sent message now shows **Reply**, **Reply all**, and **Forward** within a few seconds, so you can reply to everyone again without reopening the conversation.

## 0.147.0 — 2026-10-20

### New
- Drafts now follow you between your computers with **Replicated Sync**: start a message on one and finish it on another. Sending or discarding a draft removes it everywhere. Attached files stay on the computer where you added them, so attach them again before sending from another one. If the draft you have open changes on another computer, the composer switches to that version, or asks which to keep when you have unsaved edits.

### Improved
- **Settings → AI Provider** is simpler. Once you choose a provider, AI appears in reading, writing, and contacts, and it still sends a conversation only when you click a button marked with sparkles. Under **Show AI In**, switch off any area you don't want. **Automatic Briefs** is now one choice: **Never**, **People I've Emailed**, or **All Conversations**. Model choices and usage are under **Models and Usage**.

### Fixed
- **Remove on all devices** now also cancels messages waiting to send from that account on your other devices, so adding the account back later never sends them unexpectedly. They stay in the **Outbox** marked as failed, where you can restore them.
- **Remove on all devices** now removes an account from your other devices even when its address was typed with capital letters.
- Pictures in some newsletters and announcements now appear when **Load remote images** is on. Previously they stayed blank if the sender's server didn't label them as images.
- Event details in the calendar side panel now show attendees and the description in the same text size as the rest of the panel, and a single guest reads "1 attendee".
- Calendars on your other accounts keep loading when a calendar account you connected on another device syncs to this one. Previously **Settings → Calendar Accounts** said every account's calendars couldn't be loaded until you disconnected that account here. A newly synced account now also appears there without restarting.
- Inbox previews for some newsletters no longer show a wide blank gap followed by stray text like `&zwnj`. Conversations already in your inbox are fixed automatically.
- Escape now closes search after you open a result. Previously it worked only while the search box had focus.

## 0.145.2 — 2026-10-19

### New
- The search box now suggests as you type, the way Gmail does: type `from:` or `to:` to pick someone from your contacts, `label:` or `in:` to pick a label or folder, or `after:` to pick a date like **Past month**. An empty search box lists your recent searches. Choosing someone with several email addresses searches all of them.
- The **Trust** section now says **Be careful** when a subject line disguises its words with look-alike characters, such as a lowercase l in place of a capital I ("REVlEW"), a zero in place of an O, a letter from another alphabet, or an invisible character, and it names each disguised word.
- Use keyboard shortcuts to attach files, refresh mail, create calendar events and contacts, save contacts, and open, save, or star files. **Keyboard Shortcuts** now also lists snippets, recipient-field focus, and calendar navigation.

### Improved
- In **Settings → AI Provider**, the **Reasoning Model** and **Fast Model** fields now show a searchable list of the models your provider offers, so you can type to filter and pick one instead of remembering the exact name. You can still type a model name the list doesn't include. After you switch providers, **Refresh model list** loads the new provider's models with your saved key.
- Search now looks through all your mail, including archived conversations, without clicking **Search All Mail** first. Results still in your inbox are marked **Inbox**, **Inbox Only** narrows a search to your inbox, and Trash and Spam stay out unless you search with `in:trash` or `in:spam`. The server is searched automatically once you stop typing.
- Search understands a minus to leave things out (`invoice -from:billing@example.com`) and `OR` or braces for either-or (`from:ana OR from:bo`), in the results on this computer as well as on the server.
- Mail shortcuts now act on checked conversations together, and Select All selects the conversations visible through your filters. Undo also handles pending sends and task completion, and search stays in **Files** or **Contacts** when you’re working there.
- **Settings → Signatures** now lists your default signatures first, one compact row per address with its email, and shows any alias that uses its own signature right under its account. Each signature shows its first few lines as written and which addresses use it, and deleting one tells you which addresses will lose it.
- New installations keep mail locally for 90 days by default; existing retention settings stay as they are. You can change this in **Settings → Privacy → Keep Mail on This Device For**.

### Fixed
- In replies, the trash icon now lines up with **Review** above it.
- When one part of the window runs into a problem, only that part shows **Something went wrong** with **Try Again**, and the rest of the window keeps working instead of going blank. Choosing another conversation or mailbox also brings it back.
- Up and down arrow keys keep switching Settings sections after you click a section with the mouse.
- Keyboard navigation in the mail list now clears the gray hover highlight until you move or click the pointer, keeping the selected conversation clear.
- Searching for `to:` someone now also finds mail where they were on Cc or Bcc, as it does in Gmail.
- Trashed conversations now follow your local mail retention setting, even when starred. Other starred conversations stay on your device, and local cleanup leaves mail on the server.
- With **Load remote images** on, images now load in emails from senders whose image servers turn away anything that doesn't look like a web browser. Images are still fetched through ThreeStrands, so senders still don't see your browser, cookies, or which message you're reading from.
- Newsletters and other emails that center their content in a column now appear centered, as they do in Gmail, instead of hugging the left edge of the reading pane.

## 0.141.0 — 2026-10-16

### New
- Set a **Drive Time Buffer** at the bottom of **Settings → Availability** to reserve time before and after calendar events with a location. Meeting suggestions and conflict checks leave that time free for travel.
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
