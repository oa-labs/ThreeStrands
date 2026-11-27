---
title: Compatibility and Limitations
nav: Compatibility
summary: Check mail, calendar, platform, recording, Android, offline, and delivery support before setting up.
reviewed: next
---

## Platforms {#platforms}

macOS has Apple Silicon and Intel installers and is the platform used and tested regularly. If a release ever lacks the Intel installer, its notes link to the latest release that has one, and Intel Macs get its changes with the next release that includes them. Windows x86-64 and Linux x86-64 builds are experimental; a release may omit them if their builds fail. Choose an installer from the release's assets, not an updater payload. See [installation](getting-started.html#install-the-app).

Safe attachment previews are available on macOS and Windows. Linux does not currently provide those native previews.

- **Meeting Recording:** Available on **macOS only** (up to 4 hours per recording; stops below 2 GB free disk space). Recording system audio alongside your microphone requires **macOS 14.4 or later**; earlier macOS versions record the microphone only. See [Meeting Recording](meeting-recording.html).
- **Android:** [ThreeStrands for Android](android.html) (ARM64, Android 9 / API 28 and later) is an offline companion for **Daily Notes**, **Notes**, and **Tasks** that pairs with desktop over an **HTTPS S3** sync connector. Mail, Calendar, Contacts, AI, and Meeting Recording are desktop-only; note attachment files open on desktop.

## Providers {#providers}

| Feature | Available connection | Limitations |
| --- | --- | --- |
| Mail | Gmail sign-in; **Gmail with an app password**; IMAP/SMTP passwords or app passwords | **Switch to Gmail via IMAP…** converts an existing Gmail account per device; no dedicated Microsoft 365 sign-in |
| Proton Mail | Local Proton Mail Bridge | Bridge must be running; use its credentials |
| Calendar | Google Calendar | Separate connection from mail; no CalDAV connection |
| Contacts | Local profiles; CSV and vCard import/export; read-only **Mac Contacts** (macOS) | No ongoing Google Contacts or CardDAV synchronization |
| AI | Supported providers or a compatible custom endpoint | Optional; requires your own key and provider access |

IMAP support does not mean every provider's login method is supported. Check whether the provider allows password or app-password IMAP and SMTP access. Gmail via IMAP uses Gmail's conversation grouping when the server supplies it, and supports creating, renaming, and deleting native Gmail labels; general IMAP servers support folder management (**Manage Folders…**) and keyword labels where permitted, with structural edits performed on the account's **IMAP Editing Device** under Replicated Sync.

## Offline and Background Behavior {#offline-and-background-behavior}

Cached mail and local search can work offline. Server search, new mail, sending, calendar updates, and AI requests need their respective connections. Local mail actions wait to sync when disconnected. Retention limits how much older mail is cached.

The app catches up when it starts or resumes; it does not keep polling while closed or suspended. Send Later requires the original computer to be awake, online, authorized, and running ThreeStrands. Missed scheduled times require review. Snoozed mail returns automatically after ThreeStrands reconnects, following a check of its current provider state; any running, connected installation that received the encrypted snooze schedule can handle the return.

## Transfers and Address Books {#transfers-and-address-books}

Settings export does not contain mail, tasks, goals, notes, meeting recordings, drafts, or queued sends. Replicated Sync shares application data, while drafts use the mail provider and meeting recordings stay on the Mac where they were recorded. Contact CSV/vCard export includes phone numbers, excludes photos, groups, and Keep in Touch settings, and is unencrypted. See [the transfer matrix](settings-and-sync.html#where-your-data-goes).
