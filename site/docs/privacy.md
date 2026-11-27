---
title: Privacy and Local Storage
nav: Privacy and Local Storage
summary: Understand cached mail, remote images, meeting recordings, Mac Contacts, credentials, and what leaves your computer.
reviewed: next
---

## Keep Mail on This Device {#keep-mail-on-this-device}

Open **Settings → Privacy → Keep Mail on This Device For**. New installations default to **90 Days**; existing choices are preserved. Choose **30 Days**, **90 Days**, **1 Year**, or **Forever**.

Conversations whose newest message is older than the chosen period are removed from the local cache, including Trash. Starred conversations are kept unless they are in Trash. Mail stays on the server. This is local cleanup, not a server deletion rule.

Older messages may need an online [server search](search.html#local-results-and-older-mail) to appear again. Files lists attachments from locally synced mail; starring a file does not retain a separate offline copy. The mail-retention setting is not a promise to erase every kind of application data, exported file, or completed-send history.

## Remote Images {#remote-images}

Remote images are blocked by default. Choose **Load images** for a message, or enable **Settings → Privacy → Load Remote Images Automatically**. Loading images makes network requests to retrieve sender-hosted content through the app's native fetch path (refusing redirects to local or private network addresses). Embedded inline (`data:`) images display locally without making network requests. Do not treat loading images or a familiar sender indicator as proof that mail is safe.

## Meeting Recording {#meeting-recording}

On macOS, **Settings → Meeting Recording → Record Meetings** is off by default. Turning it on asks for **Microphone** access (**System Settings → Privacy & Security → Microphone**) and, on **macOS 14.4 or later**, **System Audio** capture permission (no Screen Recording permission is needed; Macs before 14.4 record the microphone only).

- Recordings stay strictly on your Mac in `~/Library/Application Support/app.threestrands.mail/recordings/` with owner-only permissions. Audio files and recording settings never sync across devices and are excluded from settings exports.
- **Keep Audio** defaults to **7 Days** (choices: **7 Days**, **30 Days**, **Until Deleted**, **Until Transcribed**). Use **Delete All Audio** or **Delete All Recordings** under **Recordings on This Mac** to remove files manually.
- **Remind Me to Tell Participants** stays on by default; ThreeStrands never posts to meeting chats or calendar invitations. See [Meeting Recording](meeting-recording.html).

## Credentials and Local Data {#credentials-and-local-data}

Mail cache and search data live on this computer. Sign-in tokens, mail passwords, and AI API keys use the operating system's credential store. Local-first does not mean every local file is encrypted by ThreeStrands; use your operating system's device protection for local data.

Mail and calendar connections contact their providers. Optional [AI](ai.html#what-is-shared) sends selected context to your chosen AI provider (including reviewed sources for **Prepare Agenda…**, respecting each note's **Exclude from AI** setting, and candidate emails for **Inbox Assistants**). On macOS, optional read-only **Mac Contacts** reads selected Apple Contacts address books (names, email addresses, phone numbers, companies, and job titles; managed in **System Settings → Privacy & Security → Contacts**); ThreeStrands never edits Apple Contacts, and the connection and downloaded entries stay on that Mac while saved profiles follow your sync and export choices. [Encrypted sync](sync.html) uses storage you choose, and [settings export](settings-and-sync.html) creates a password-encrypted file. Contact CSV and vCard exports are unencrypted.

## Remove Everything From This Computer {#remove-everything}

Uninstalling ThreeStrands removes the app, but not your mail, settings, or saved sign-ins. To remove all of it:

1. **Remove your accounts.** In **Settings → Mail Accounts** and **Settings → Calendar Accounts**, remove each account. Removing a mail account deletes everything ThreeStrands kept for it on this computer, including its mail, drafts, Outbox, attachments you opened, and older backup copies of your mail. Removing a Gmail or Google Calendar account also ends ThreeStrands' access to it. With [Replicated Sync](sync.html), **Disconnect this device** leaves your other computers connected, and Google access stays on for them. **Remove on all devices** removes the account everywhere.
2. **Quit ThreeStrands and uninstall it.**
3. **Delete its folders** and **its saved sign-ins**, listed below for your system.
4. **Check Google access.** If you no longer use ThreeStrands on any computer, open your [Google Account permissions](https://myaccount.google.com/permissions) and remove ThreeStrands if it is still listed.
5. **Delete synced data.** If you used Replicated Sync and no computer uses it any more, delete the ThreeStrands files in the storage location you chose.

Exported files, such as settings exports and contact CSV or vCard files, stay wherever you saved them.

### macOS {#remove-everything-macos}

In Finder, choose **Go → Go to Folder**, and delete each of these folders that exists:

- `~/Library/Application Support/app.threestrands.mail`: mail, contacts, notes, meeting recordings (`recordings/`), backups, and attachments
- `~/Library/Logs/app.threestrands.mail`: logs
- `~/Library/WebKit/app.threestrands.mail`, `~/Library/Caches/app.threestrands.mail`, and `~/Library/HTTPStorages/app.threestrands.mail`: window settings and crash reports

Then open **Keychain Access**, search for `threestrands`, and delete every item it finds.

### Windows {#remove-everything-windows}

When you uninstall, check **Delete the application data**. Then paste each of these into File Explorer's address bar, and delete the folder if it is still there:

- `%APPDATA%\app.threestrands.mail`: mail, contacts, notes, backups, and attachments
- `%LOCALAPPDATA%\app.threestrands.mail`: logs, window settings, and crash reports

Then open **Credential Manager**, choose **Windows Credentials**, and remove every generic credential whose name contains `app.threestrands`, such as `you@gmail.com.app.threestrands.mail`.

### Linux {#remove-everything-linux}

Delete `~/.local/share/app.threestrands.mail`, which holds mail, contacts, notes, backups, attachments, logs, and window settings. If you set `XDG_DATA_HOME`, the folder is under that directory instead. If you made the AppImage your default mail app, also delete `~/.local/share/applications/app.threestrands.mail.desktop`.

Then open your keyring app, such as **Passwords and Keys**, search for `threestrands`, and delete every item it finds. Sign-ins are stored with Secret Service, labeled like `keyring:you@gmail.com@app.threestrands.mail`.

## Sender and Attachment Checks {#sender-and-attachment-checks}

Checks run locally and explain suspicious addresses, display-name mismatches, new senders (including newsletters and bulk mail), links, and attachments. Trusting a sender remembers their link destinations for one year from your last confirmation and adds newly confirmed sites as you trust later messages. Choosing **Mark Spam** also blocks the sender (forgotten after six months of silence; review or unblock under **Settings → Muted Senders → Blocked Senders**). Opening shortcut or launcher attachments (`.webloc`, `.inetloc`, `.fileloc` on macOS; `.rdp`, `.searchConnector-ms`, `.theme`, `.website` on Windows) asks for confirmation, and exporting notes applies OS quarantine or download marks to exported attachments. A known sender is not a promise that every message is safe. Read [Trust and Attachment Warnings](triage.html#trust-and-attachment-warnings) before confirming a new sending address.

## Diagnostics and Reports {#diagnostics-and-reports}

**Settings → Diagnostics** shows sync health and local crash-report controls. **Share Sanitized Crash Reports** is **off by default**; when enabled, reports include the error message, stack trace, app version, and browser engine, with email addresses, URLs, quoted strings, and message headers (`Subject`, `To`, `Cc`, `Bcc`, `From`) redacted automatically (up to 20 reports kept locally, cleared via **Clear N local reports**). Review any error or report before sharing it publicly: account details and message identifiers may still identify private activity. See [Get Help](get-help.html) and the [Privacy Policy](../privacy.html) for more detail.
