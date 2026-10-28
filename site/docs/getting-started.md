---
title: Getting Started
nav: Getting Started
summary: Install ThreeStrands, connect Gmail or an IMAP account, and set up the few things worth changing on day one.
reviewed: 0.179.0
---

## Install the App {#install-the-app}

Download the latest release from [GitHub Releases](https://github.com/oa-labs/ThreeStrands/releases/latest):

- **macOS** — a signed and notarized `.dmg` for Apple Silicon and Intel Macs.
- **Windows (experimental)** — an x86-64 `.exe` installer, when included in the release.
- **Linux (experimental)** — `.deb`, `.rpm`, and `.AppImage` packages for x86-64.

macOS is the platform used and tested regularly. Experimental Windows and Linux installers may be absent when a release build fails. Choose the installer, not an updater signature or archive. On macOS, open the DMG and move ThreeStrands to **Applications** before launching. For Linux, choose the package for your distribution or make the AppImage executable. See [Compatibility](compatibility.html).

ThreeStrands stores your mail cache in a local database on your computer. Sign-in tokens and passwords go to your operating system's credential store, never to a plain file.

## Connect Gmail {#connect-gmail}

1. Open **Settings** with <kbd>⌘/Ctrl + ,</kbd>, or click **Add Account** in the empty inbox.
2. In **Mail Accounts**, click **Add Account**, then choose **Gmail**.
3. Sign in to Google in your browser. The button reads **Waiting for Google…** until you finish.
4. When the browser says *ThreeStrands is connected*, close the tab. Your mail starts syncing.

## Connect an IMAP Account {#connect-an-imap-account}

Choose **IMAP** instead of Gmail to connect any IMAP/SMTP server.

1. Enter your **Email Address** and click **Find Settings Automatically**. If your provider publishes its settings, ThreeStrands fills in the servers for you.
2. Otherwise, fill in **Incoming Mail (IMAP)** and **Outgoing Mail (SMTP)**: host, port, and security. **STARTTLS** defaults to ports 143 and 587, and **Implicit TLS** to 993 and 465.
3. Enter the **Password**. Many providers want an app password here rather than your normal sign-in password.
4. Click **Test and Save**. Both servers must connect before the account is added.
5. In **Map Your Mailboxes**, confirm which folders are Sent, Archive, Drafts, Trash, and Junk. Mappings marked *name guess — please verify* are worth a second look.

### Proton Mail Bridge {#proton-mail-bridge}

Proton Mail needs the local Bridge application and its own credentials. Follow [Connect Proton Mail Bridge](proton-bridge.html) for server settings, certificate checks, and recovery.

## Make Your Accounts Recognizable {#make-your-accounts-recognizable}

In **Settings → Mail Accounts**, each account has:

- **Account Nickname** — a short name such as *Work* or *Personal*, used on account icons.
- **Sender Name** — the name recipients see in the From line.
- **A color** — click the account's avatar to change it.
- **Order** — with two or more accounts, move them up or down. <kbd>⌘/Ctrl + 1</kbd> through <kbd>⌘/Ctrl + 9</kbd> switch accounts in this order, and <kbd>⌘/Ctrl + 0</kbd> shows all accounts together.

## Settings Worth a Look {#settings-worth-a-look}

- **Appearance** — theme, accent color, font size, and when conversations are marked read. **Email Zoom** enlarges email text, images, and spacing together, and **Email Text Colors** lightens or darkens hard-to-read sender text just enough to read, or replaces it with your theme's colors. Press <kbd>⌘/Ctrl + d</kbd> to switch between light and dark mode from anywhere. Appearance settings stay on each computer.
- **Availability** — your timezone, working-hour blocks with gaps for breaks, and **Drive Time Buffer** around events with a location.
- **Signatures** — formatted sign-offs with logos, Gmail import, and separate defaults for new messages and replies.
- **Muted Senders** — review senders whose new mail is marked read and archived automatically.
- **Privacy** — remote images are blocked by default; turn on **Load Remote Images Automatically** here, or click **Load images** on a single message. You can also choose how long to keep mail on this device.
- **Default Apps** — open email links and `.ics` invitations in ThreeStrands.

## Updates {#updates}

The app offers newer releases when available. Follow its **Install Update** or download action. macOS copies outside Applications may ask you to move the app there first. Updates can restart the app; resolve urgent unsent work first. If installation fails, keep the error and download the current installer from Releases instead of deleting local data. Package-based Linux installs may require downloading a replacement package.

## After Connecting {#after-connecting}

Let the initial sync run. Locally available mail and server search are different: older messages may need an online search. New installations keep a 90-day local cache by default; [retention cleanup leaves server mail alone](privacy.html#keep-mail-on-this-device).

Next: [your first ten minutes](first-steps.html), or [troubleshoot a connection](troubleshooting.html#mail-stopped-arriving).
