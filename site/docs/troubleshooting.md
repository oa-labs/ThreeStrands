---
title: Troubleshooting
nav: Troubleshooting
summary: Check mail, sending, drafts, calendar, and sync problems before removing data.
reviewed: 0.179.0
---

## Mail Stopped Arriving {#mail-stopped-arriving}

1. Confirm which account is selected and whether the computer is online. ThreeStrands polls while running and catches up on launch or resume; it cannot poll while closed or suspended.
2. Check the inbox's last-check status and **Settings → Diagnostics → Sync Health** for the last successful sync and errors.
3. If authentication needs attention, reconnect in **Settings → Mail Accounts**. For Bridge, keep Bridge running and recheck its current credentials and certificate.
4. After fixing the cause, use **Retry** for rejected changes in Diagnostics. **Dismiss** clears a reported problem; it does not make the server apply it.

Expect last-successful-sync status to update after a successful connection. If the provider's own app also cannot find new mail, investigate the provider connection. [Get help](get-help.html) with persistent errors instead of deleting the local database.

## A Message Is Missing {#a-message-is-missing}

Check the account selection and **Clear Filters**, then search all mail rather than only Inbox. Archived mail is still searchable. Include `in:trash`, `in:spam`, or `in:anywhere` when appropriate. Older mail may have left the local cache, so stay online for server search results. See [Search](search.html).

If an attachment is missing from Files, find its source email first. File stars do not keep a separate offline copy. Mail-retention cleanup leaves server mail alone.

## A Message Will Not Send {#a-message-will-not-send}

Open **Outbox** from the folder menu or with <kbd>g</kbd> then <kbd>o</kbd>. Read the selected item's delivery status before choosing an action.

- Waiting for its scheduled time: keep the original computer awake, online, authorized, and running ThreeStrands.
- Missed scheduled time: review and confirm sending; another computer will not take over.
- Failed: address the reported account, network, or SMTP problem, then use the offered recovery action or return it to drafts.
- Delivery uncertain: choose **Check Sent Mail** and check your provider's Sent folder before sending a duplicate.

Finished sends move to **Sent**. See [Outbox](compose.html#outbox). Do not remove the account as a first troubleshooting step.

## A Draft Is Missing or Conflicted {#a-draft-is-missing-or-conflicted}

Check **Drafts** in ThreeStrands and at your provider. Gmail and mapped IMAP Drafts can reach other devices; an IMAP account without a mapped Drafts mailbox keeps drafts locally. Allow time for the provider to sync while online.

When a draft changed both here and elsewhere, review the composer's version choices before continuing. Keep a copy of needed text before discarding a version. See [Writing Mail](compose.html#start-a-message).

## Calendar Availability Looks Wrong {#calendar-availability-looks-wrong}

In **Settings → Calendar Accounts**, distinguish **Calendars shown in the sidebar** from **Calendars used for availability**. Check **Settings → Availability** for timezone, working-hour blocks, and Drive Time Buffer. Check the date range and morning/afternoon filters in Find Times.

**Availability unknown** or **Calendar check details** means not every calendar could be checked; it does not mean a guest is free. Reconnect the named account or review access permissions before relying on the times. See [Calendar](calendar.html).

## Sync Is Not Reaching Another Computer {#sync-is-not-reaching-another-computer}

Both computers need encrypted sync enabled, a reachable connector for the same group, and completed device approval. Check each computer's connector and device status in **Settings → Replicated Sync**. Join the existing group rather than creating a separate one.

Mail accounts still require separate sign-in, and drafts use provider Drafts instead of replicated sync. Appearance and file stars stay local. Use [Where Your Data Goes](settings-and-sync.html#where-your-data-goes) to check whether the item should synchronize at all. Preserve local data and the recovery phrase while investigating; never publish the phrase or storage credentials.

## Email Is Difficult to Read {#email-is-difficult-to-read}

Try **Email Zoom**, **Minimum Email Font Size**, and **Email Text Colors** in **Settings → Appearance**. Images are controlled separately under **Settings → Privacy**. Small text baked into an image will not change with a font setting. See [Appearance and Accessibility](appearance.html).

## AI Is Unavailable or Failing {#ai-is-unavailable-or-failing}

Open **Settings → AI Provider**. Confirm the provider, saved key, model name, and **Show AI In** choices, then use **Test Connection**. Usage or account limits at the provider can stop requests. A provider subscription and an API billing account may be different products; check the account associated with your API key.

For a custom endpoint, verify its URL and compatible model. Keep keys out of reports. See [AI Assistance](ai.html).

## The App Reports a Problem {#the-app-reports-a-problem}

**Try Again** can recover a failed pane; changing conversation or mailbox may also recover it. Diagnostics can show rejected changes, unreadable messages, and local cache recovery. Read the explanation before dismissing it. For update errors, [check the installation guidance](getting-started.html#updates).

If these checks do not resolve the problem, [report it with version, platform, steps, and a redacted error](get-help.html#report-a-problem).
