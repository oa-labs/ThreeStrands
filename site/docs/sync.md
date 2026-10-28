---
title: Sync Across Computers
nav: Sync Across Computers
summary: Set up encrypted sync, add another device, and understand what stays local.
reviewed: 0.179.0
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

1. Install ThreeStrands and open **Settings → Replicated Sync** there.
2. Add the same connector and let the app check for the existing group.
3. Join with the recovery phrase, or use a join code from **Add a device** on an existing computer.
4. For the join-code flow, compare the codes on both screens and approve only if they match.
5. Check the sync status, then use **Settings → Mail Accounts → On Your Other Devices → Set Up Here** for accounts you want connected. Sign in separately to mail and calendar providers.

If tasks or contacts do not arrive, check [sync troubleshooting](troubleshooting.html#sync-is-not-reaching-another-computer) before creating another group.

## What Syncs {#what-syncs}

Tasks, goals, saved contacts and groups, snippets, signatures and their defaults, Writing Profile, muted senders, sender-trust confirmations, Split Inboxes, account details, and portable preferences use encrypted sync. Appearance stays on each computer. Drafts travel through the mail provider's Drafts mailbox instead.

Scheduled mail stays owned by its original computer. Other devices see a read-only summary, not the body, recipients, or attachments, and never take over delivery. Compare all mechanisms in [Where Your Data Goes](settings-and-sync.html#where-your-data-goes).

## Revoke or Leave {#revoke-or-leave}

Use the device controls in **Settings → Replicated Sync** to revoke a device you no longer want participating, or leave the group from this computer. Revocation does not erase copies a device already received. Leaving a group does not delete mail at your provider.

Deleting connector files does not erase copies on other devices, other connectors, or storage-provider version history. A shared sync folder is not a complete backup of your computer.

## Recover Access {#recover-access}

Keep a working device available while replacing a computer. Use it to approve the new device, or use the recovery phrase. If you lose the phrase but still have an authorized device, use the add-device flow before retiring that device. If no authorized device or phrase remains, do not expect support to decrypt the existing group. Preserve remaining local data and [ask for help](get-help.html) before deleting connector storage or starting over.
