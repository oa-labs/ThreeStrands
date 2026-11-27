---
title: Sync Across Computers
nav: Sync Across Computers
summary: Set up encrypted sync, pair another computer or Android phone, and understand what stays local.
reviewed: next
---

## Before You Start {#before-you-start}

Replicated Sync shares application data through storage you choose. It does not sign in to mail accounts for you, copy your mailbox, or make another computer take over scheduled delivery. For a one-time settings move, use [Settings Transfer](settings-and-sync.html).

Use the same current app version on your computers. Choose a shared folder for the simplest setup if you already use a folder-sync service. S3-compatible storage and IPFS need a service or node you can configure and reach.

## Set Up the First Computer {#set-up-the-first-computer}

1. Open **Settings → Replicated Sync** and enable replicated sync.
2. **Add a connector**: a **Shared folder**, **S3 storage**, or **IPFS** endpoint.
3. For a shared folder, choose a folder your existing service synchronizes. For S3 or IPFS, enter the connection settings for your storage.
4. If no group exists, choose **Create a new sync group**. If a group already exists, join it instead of creating a separate group.
5. Store the 24-word recovery phrase securely. It is shown only once. Never put it in an issue report or send it to support.

## Add Another Computer {#add-another-computer}

Install ThreeStrands on the new computer, open **Settings → Replicated Sync**, turn on **Enable replicated sync**, and choose one of three ways to join:

- **Join with a code from another device (fastest):** On an existing synced computer, open **Devices → Add a device**, pick which connectors and credentials to include, and click **Create join code**. Join codes are single-use, masked by default (**Show** / **Hide**), and grant access to your full sync history (and any included storage credentials, which do not expire when the code expires); cancel an unused code under **Join Codes → Cancel…** to rotate keys. On the new computer, paste the code under **Join with a code from another device** (no need to add the connector first; for a shared folder just pick that computer's copy of the folder, and for S3/IPFS enter credentials only if omitted), then click **Join sync group**. The inviting computer admits the new device automatically on its next sync (no fingerprint check), and other devices show a notice with **Not you? Revoke…**.
- **Ask another device to approve this one:** Add the same connector on the new computer first, then click **Ask another device to approve this one**. On an existing computer under **Devices waiting to join**, click **Review…**, compare the fingerprint codes on both screens, and click **Codes match — approve**; then click **Check for approval** and **Confirm — fingerprints match** on the new computer.
- **Join with the recovery phrase:** Add the same connector on the new computer first, then enter your 24-word recovery phrase and click **Join with recovery phrase**.

After joining, check the sync status, then use **Settings → Mail Accounts → On Your Other Devices → Set Up Here** for accounts you want connected. Sign in separately to mail and calendar providers.

> **Updating from an older sync protocol:** To harden recovery-phrase joins, updating to the current sync format automatically leaves any earlier group while keeping your local data and connectors intact. Update every device first, create a new sync group on one computer (save the new 24-word recovery phrase), delete legacy group files via **Delete files and disconnect** if the connector still holds an older group, and join from your other devices.

If tasks, notes, or contacts do not arrive, check [sync troubleshooting](troubleshooting.html#sync-is-not-reaching-another-computer) before creating another group.

## Pair an Android Phone {#pair-an-android-phone}

ThreeStrands for Android syncs **Daily Notes**, **Notes**, and **Tasks** over an **HTTPS S3** connector:

1. On desktop under **Settings → Replicated Sync → Devices → Add a device**, select **only your HTTPS S3 connector** and create a join code.
2. On Android, open **Settings → Sync settings**, paste the code, tap **Review Join Code**, enter S3 credentials if omitted, and tap **Join Sync Group**.
3. Sync once more on desktop so it admits the phone, then reopen Android or tap **Sync Now**. On desktop, choose **All accounts** in **Tasks** to view standalone tasks created on the phone.

See [ThreeStrands for Android](android.html) for backup export and disconnect options.

## What Syncs {#what-syncs}

Tasks (including manual task board card order), goals, notes (including Daily Notes, attached pictures/files, and each note's **Exclude from AI** setting), saved contacts (including phone numbers) and groups, snippets, signatures and their defaults, Writing Profile, muted and blocked senders, sender-trust confirmations, conversation snooze schedules, Split Inboxes, account details, and portable preferences use encrypted sync. Appearance, **Meeting Recording** settings and audio (macOS), **Inbox Assistants**, **Mac Contacts** downloaded entries, file stars, and searches stay on each computer. Drafts travel through the mail provider's Drafts mailbox instead.

Concurrent edits to the same note appear under **Resolve Conflicts** in **Settings → Replicated Sync** with **Open Note** so you can compare versions in **Notes**. Switching a Gmail account with **Switch to Gmail via IMAP…** works one device at a time while keeping shared tasks, goals, snoozes, and Split Inboxes; for IMAP accounts, label and folder renames/deletions run from the account's designated **IMAP Editing Device** (**Settings → Mail Accounts → ⋯ → IMAP Editing Device…**).

Scheduled mail stays owned by its original computer. Other devices see a read-only summary, not the body, recipients, or attachments, and never take over delivery. Compare all mechanisms in [Where Your Data Goes](settings-and-sync.html#where-your-data-goes).

Snooze schedules share account, conversation, and message references and the return time; they do not carry email content. A connected installation can return due conversations after receiving the schedule. Make sure all devices are updated to the same current release before syncing snoozes, notes, or newer entity types.

## Revoke or Leave {#revoke-or-leave}

Use the device controls in **Settings → Replicated Sync** to revoke a device you no longer want participating, or leave the group from this computer. Revocation does not erase copies a device already received. Leaving a group does not delete mail at your provider.

Deleting connector files does not erase copies on other devices, other connectors, or storage-provider version history. A shared sync folder is not a complete backup of your computer.

## Recover Access {#recover-access}

Keep a working device available while replacing a computer. Use it to approve the new device, or use the recovery phrase. If you lose the phrase but still have an authorized device, use the add-device flow before retiring that device. If no authorized device or phrase remains, do not expect support to decrypt the existing group. Preserve remaining local data and [ask for help](get-help.html) before deleting connector storage or starting over.
