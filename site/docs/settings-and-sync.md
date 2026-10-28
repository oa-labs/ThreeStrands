---
title: Settings Transfer and Sync
nav: Settings Transfer
summary: Move selected settings safely, compare transfer with sync, and replace a computer.
reviewed: 0.179.0
---

## Export and Import Settings {#export-and-import-settings}

Use an encrypted export to move selected settings or keep a settings backup. **It is not a backup of mail, drafts, tasks, goals, or queued sends.** Mail and drafts belong to the mail provider; use [encrypted sync](sync.html) for tasks and goals.

The file includes preferences, account list, Split Inboxes, retention setting, snippets, signatures and their defaults, each account's **Writing Profile**, muted senders, and saved contacts and groups. It never includes sign-in tokens, passwords, or API keys.

**Before importing:** import replaces the destination's preferences, Split Inboxes, signatures and defaults, snippets, saved contacts and groups, muted senders, and retention setting. Existing connected accounts stay connected. Export the destination's settings first if you may need to restore them.

1. On the source computer, open **Settings → Data Transfer**.
2. Enter and confirm an **Export Password** of at least eight characters, then click **Export Encrypted Settings**. Keep the file and password secure; you need both to import.
3. On the destination, enter the **Backup File Password** and click **Choose Encrypted Settings File**.
4. If offered account choices, select which imported accounts to set up here and choose **Import**. When all accounts are already connected, import can apply without that selection screen.
5. Authenticate newly selected accounts. Accounts you do not connect appear in **Settings → Mail Accounts → On Your Other Devices**; use **Set Up Here** later if needed.

<ts-transfer-contents></ts-transfer-contents>

## Where Your Data Goes {#where-your-data-goes}

“Provider” means your mail or calendar service. “Encrypted sync” means ThreeStrands Replicated Sync. An export is a snapshot, not ongoing synchronization.

| Data | Provider | Encrypted sync | Settings export | Device only |
| --- | --- | --- | --- | --- |
| Mail and mailbox changes | Yes | No | No | Local cache |
| Drafts and attached files | Provider Drafts, when mapped | No | No | Local fallback if no Drafts mailbox |
| Calendar events | Google Calendar | No | No | Cached view |
| Tasks and goals | No | Yes | No | Until sync is enabled |
| Saved contacts and groups | No ongoing address-book sync | Yes | Yes | Until transferred or synced |
| Snippets, signatures, Writing Profile, muted senders, Split Inboxes | No | Yes | Yes | Until transferred or synced |
| Sender-trust confirmations | No | Yes, for the same account | No | Until synced |
| Appearance | No | No | Yes | Ongoing choices stay local |
| Account details and portable preferences | No | Yes | Yes | Credentials excluded |
| Credentials and AI API keys | Used to sign in | No | No | OS credential store |
| File stars and recent searches | No | No | No | Yes |
| Scheduled messages | Sent by original computer | Read-only summaries | No | Body, recipients, attachments and sending ownership |

## Replicated Sync {#replicated-sync}

Replicated Sync keeps application data in step across computers using end-to-end encryption and storage you choose. Drafts reach your other computers through your mail account's Drafts instead. See [Sync Across Computers](sync.html) for the full setup, join-code verification, recovery, and device-removal procedures.

> Scheduled mail is still sent only by the computer where it was scheduled. Other computers in the group see a read-only summary.

## Replace a Computer {#replace-a-computer}

1. Keep the old computer available. Check that drafts have reached the provider and that encrypted sync is healthy if you use it.
2. Review **Outbox** on the old computer. Resolve scheduled or uncertain sends there; another computer will not take them over. Check Sent before recreating a message with uncertain delivery.
3. Export settings if needed, then import them on the replacement, or join the existing sync group. Use the matrix above to decide which data each method covers.
4. Sign in to mail and calendar accounts separately, and set up your AI key again if wanted.
5. Confirm that the replacement has the tasks, contacts, drafts, and accounts you need before retiring the old device. Review device revocation in [Sync](sync.html#revoke-or-leave).

Do not assume an encrypted settings file or sync connector contains every local artifact. Preserve any needed local-only data separately.
