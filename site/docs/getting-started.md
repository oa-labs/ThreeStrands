---
title: Getting Started
nav: Getting Started
summary: Install ThreeStrands, connect Gmail or an IMAP account, and set up the few things worth changing on day one.
---

## Install the App

Download the latest release from [GitHub Releases](https://github.com/oa-labs/dispatch/releases/latest):

- **macOS** — a signed and notarized `.dmg` for Apple Silicon and Intel Macs.
- **Linux** — `.deb`, `.rpm`, and `.AppImage` packages for x86-64.

ThreeStrands stores your mail cache in a local database on your computer. Sign-in tokens and passwords go to your operating system's credential store, never to a plain file.

## Connect Gmail

1. Open **Settings** with <kbd>⌘/Ctrl + ,</kbd>, or click **Add Account** in the empty inbox.
2. In **Mail Accounts**, click **Add Account**, then choose **Gmail**.
3. Sign in to Google in your browser. The button reads **Waiting for Google…** until you finish.
4. When the browser says *ThreeStrands is connected*, close the tab. Your mail starts syncing.

## Connect an IMAP Account

Choose **IMAP** instead of Gmail to connect any IMAP/SMTP server.

1. Enter your **Email Address** and click **Find Settings Automatically**. If your provider publishes its settings, ThreeStrands fills in the servers for you.
2. Otherwise, fill in **Incoming Mail (IMAP)** and **Outgoing Mail (SMTP)**: host, port, and security. **STARTTLS** defaults to ports 143 and 587, and **Implicit TLS** to 993 and 465.
3. Enter the **Password**. Many providers want an app password here rather than your normal sign-in password.
4. Click **Test and Save**. Both servers must connect before the account is added.
5. In **Map Your Mailboxes**, confirm which folders are Sent, Archive, Drafts, Trash, and Junk. Mappings marked *name guess — please verify* are worth a second look.

### Proton Mail Bridge

Proton Mail works through [Proton Mail Bridge](https://proton.me/mail/bridge), which runs on your computer. Bridge can't be found automatically, so enter its settings by hand:

1. In Bridge, open your account's mailbox details to see its IMAP and SMTP settings.
2. Copy the host (usually `127.0.0.1`), both ports, the security mode, the username, and the Bridge password into ThreeStrands. Use the Bridge password, not your Proton password.
3. Bridge uses its own certificate. Click **Check IMAP Certificate** (and **Check SMTP Certificate**), review it, and choose **Trust This IMAP Certificate for This Server** before **Test and Save**.

## Make Your Accounts Recognizable

In **Settings → Mail Accounts**, each account has:

- **Account Nickname** — a short name such as *Work* or *Personal*, used on account icons.
- **Sender Name** — the name recipients see in the From line.
- **A color** — click the account's avatar to change it.
- **Order** — with two or more accounts, move them up or down. <kbd>⌘/Ctrl + 1</kbd> through <kbd>⌘/Ctrl + 9</kbd> switch accounts in this order, and <kbd>⌘/Ctrl + 0</kbd> shows all accounts together.

## Settings Worth a Look

- **Appearance** — theme, accent color, font size, and when conversations are marked read.
- **Availability** — your timezone and working hours, used when you share free times.
- **Privacy** — remote images are blocked by default; turn on **Load Remote Images Automatically** here, or click **Load images** on a single message. You can also choose how long to keep mail on this device.
- **Default Apps** (macOS) — open email links and `.ics` invitations in ThreeStrands.

Next: [learn the triage loop](triage.html).
