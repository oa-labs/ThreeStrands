---
title: Compatibility and Limitations
nav: Compatibility
summary: Check mail, calendar, platform, offline, and delivery support before setting up.
reviewed: 0.179.0
---

## Platforms {#platforms}

macOS has Apple Silicon and Intel installers and is the platform used and tested regularly. Windows x86-64 and Linux x86-64 builds are experimental; a release may omit them if their builds fail. Choose an installer from the release's assets, not an updater payload. See [installation](getting-started.html#install-the-app).

Safe attachment previews are available on macOS and Windows. Linux does not currently provide those native previews.

## Providers {#providers}

| Feature | Available connection | Limitations |
| --- | --- | --- |
| Mail | Gmail sign-in; IMAP/SMTP passwords or app passwords | Authentication requirements depend on the provider; no dedicated Microsoft 365 sign-in |
| Proton Mail | Local Proton Mail Bridge | Bridge must be running; use its credentials |
| Calendar | Google Calendar | Separate connection from mail; no CalDAV connection |
| Contacts | Local profiles, CSV and vCard import/export | No ongoing Google Contacts or CardDAV synchronization |
| AI | Supported providers or a compatible custom endpoint | Optional; requires your own key and provider access |

IMAP support does not mean every provider's login method is supported. Check whether the provider allows password or app-password IMAP and SMTP access.

## Offline and Background Behavior {#offline-and-background-behavior}

Cached mail and local search can work offline. Server search, new mail, sending, calendar updates, and AI requests need their respective connections. Local mail actions wait to sync when disconnected. Retention limits how much older mail is cached.

The app catches up when it starts or resumes; it does not keep polling while closed or suspended. Send Later requires the original computer to be awake, online, authorized, and running ThreeStrands. Missed scheduled times require review.

## Transfers and Address Books {#transfers-and-address-books}

Settings export does not contain mail, tasks, goals, drafts, or queued sends. Replicated Sync shares application data, while drafts use the mail provider. Contact CSV/vCard export excludes photos, groups, and Keep in Touch settings and is unencrypted. See [the transfer matrix](settings-and-sync.html#where-your-data-goes).
