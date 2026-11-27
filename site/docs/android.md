---
title: ThreeStrands for Android
nav: Android
summary: Capture Daily Notes, Notes, and Tasks offline on Android, and keep them in step with desktop through an encrypted HTTPS S3 connector.
reviewed: next
---

## What Android Includes {#what-android-includes}

ThreeStrands for Android (ARM64, Android 9 / API 28 and later) is a companion app for **Daily Notes**, **Notes**, and **Tasks**:

- Opens directly to today's **Daily Note**. Use **Capture** to append to today's note, create a **New Note**, or add a **New Task**.
- Insert square **Checklist** items (note-local) or round **Task** items (which also appear in **Tasks**, where tapping one opens its note).
- Work and search completely offline with saved drafts, undo after completing a task or moving a note to Trash, and side-by-side version comparison when a note changes on two devices before syncing.
- Note attachment references appear in note text; use desktop to open attachment files. Mail, Calendar, Contacts, AI features, and Meeting Recording are desktop-only.

## Pair With Desktop Over HTTPS S3 {#pair-with-desktop}

Android syncs with your desktop devices through an **HTTPS S3** connector in [Replicated Sync](sync.html):

1. On desktop, open **Settings → Replicated Sync**, turn on **Enable replicated sync**, and add an **S3 storage** connector with an HTTPS endpoint (**Test Connection** → **Add connector**). If this is your first synced device, create a new sync group and save its 24-word recovery phrase.
2. Under **Devices → Add a device**, select **only the S3 connector**, choose whether to include its credentials in the code, and create a join code.
3. On Android, open **Settings → Sync settings**, paste the join code, tap **Review Join Code**, enter S3 credentials if omitted, and tap **Join Sync Group**.
4. Sync once more on desktop so the inviting computer admits your phone, then reopen Android or tap **Sync Now**.

The phone joins your encrypted group while preserving the rest of the group's records. On desktop, switch **Tasks** to **All accounts** to see standalone tasks created on Android.

## Back Up or Disconnect {#back-up-or-disconnect}

- **Export Notes and Tasks:** Open **Settings → Sync settings** on Android to save a ZIP archive containing Markdown notes and a copy of your tasks. Export files are unencrypted, so store them somewhere you control.
- **Disconnect and Keep Data:** Stops syncing on the phone while keeping your local notes and tasks on the device.

See [Sync Across Computers](sync.html), [Notes](notes.html), [Tasks and Goals](tasks-and-goals.html), and [Compatibility](compatibility.html#platforms) for details.
