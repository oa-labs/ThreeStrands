---
title: Settings Transfer and Sync
nav: Settings and Sync
summary: Move your setup to a new computer with an encrypted export, or keep several computers in step with end-to-end encrypted sync.
---

## Export and Import Settings

Use an encrypted export to set up a new computer or keep a backup.

1. Open **Settings → Data Transfer**.
2. Enter an **Export Password** of at least eight characters, confirm it, and click **Export Encrypted Settings**.
3. On the other computer, enter the **Backup File Password** and click **Choose Encrypted Settings File**.

The file includes your preferences, account list, Split Inboxes, retention setting, snippets, signatures and their defaults, muted senders, and saved contacts and groups. It never includes mail, sign-in tokens, passwords, or API keys, and lets you choose which mail accounts to set up on this computer. Accounts not connected here appear in **Settings → Mail Accounts → On Your Other Devices**; choose **Set Up Here** to connect only the ones you want.

## Replicated Sync

Replicated Sync keeps tasks, goals, saved contacts and groups, snippets, signatures and their defaults, muted senders, Split Inboxes, account details, and preferences the same across your computers. It is end-to-end encrypted, and it uses storage you choose rather than a ThreeStrands server.

1. Open **Settings → Replicated Sync** and check **Enable replicated sync**.
2. **Add a connector** — choose where encrypted data is kept:
   - **Shared folder** — a folder synced by Dropbox, iCloud Drive, Syncthing, or similar.
   - **S3 storage** — Amazon S3, Cloudflare R2, Backblaze B2, Wasabi, MinIO, or another S3-compatible service.
   - **IPFS** — an IPFS node you run.
3. **Create a new sync group** and write down the 24-word recovery phrase. It's shown only once.
4. On each other computer, join with the recovery phrase or with a join code from **Add a device**, then compare the codes on both screens and approve.

You can revoke a device or leave the sync group at any time.

> Scheduled mail is still sent only by the computer where it was scheduled. Other computers in the group see a read-only summary.
