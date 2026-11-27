---
title: Meeting Recording (macOS)
nav: Meeting Recording
summary: Record meeting audio locally on your Mac from the menu bar, Calendar, or Today, and choose how long recordings are kept.
reviewed: next
---

## Requirements and Setup {#requirements-and-setup}

Meeting recording is available on **macOS only** and is **off by default**.

1. Open **Settings → Meeting Recording** and turn on **Record Meetings**.
2. Grant **Microphone** permission when macOS asks (**System Settings → Privacy & Security → Microphone**).
3. On **macOS 14.4 or later**, also allow **System Audio** capture so ThreeStrands can record the other participants alongside your microphone. No Screen Recording permission is needed. On Macs running an earlier version of macOS, ThreeStrands records your microphone only.

## Start, Pause, and Stop a Recording {#start-pause-and-stop}

Once **Record Meetings** is turned on, you can control recordings from three places:

<ts-shot scene="meeting-recording" alt="Calendar meeting details with an active recording status indicator and Stop Recording control."></ts-shot>

- **Menu bar icon:** Use **Start Recording**, **Pause Recording**, **Resume Recording**, or **Stop Recording** from the ThreeStrands menu bar icon. Closing the main window leaves ThreeStrands running in the menu bar so an active recording is not interrupted.
- **Calendar and Today:** Meetings that are under way or starting within 10 minutes show a **Record** / **Stop Recording** button in Calendar **Event details** and on **Today**.
- **Command palette:** Press <kbd>⌘ + k</kbd> and run **Start Recording**, **Pause Recording**, **Resume Recording**, or **Stop Recording**.

The first time you start a recording on a Mac, ThreeStrands shows a consent notice reminding you that recording laws vary by location. **Remind Me to Tell Participants** stays on by default and shows a notice at the start of every recording; ThreeStrands never posts messages to meeting chats or calendar invitations.

While recording, a sidebar indicator shows a status dot, elapsed time (or **Paused**), and a **Stop** button. Quitting while recording asks **Stop Recording and Quit?** before exiting. Each recording is limited to **4 hours** and stops automatically with a notice if free disk space drops below **2 GB**.

## Storage, Retention, and Deletion {#storage-retention-and-deletion}

Recordings stay strictly on your Mac in `~/Library/Application Support/app.threestrands.mail/recordings/` with owner-only file permissions. File permissions are owner-only, and audio files and **Settings → Meeting
Recording** preferences are never sent over [cross-device sync](sync.html) or
included in [settings exports](settings-and-sync.html#where-your-data-goes).

- **Keep Audio:** Choose how long recorded audio is kept (**7 Days** by default, **30 Days**, **Until Deleted**, or **Until Transcribed**).
- **Delete recordings:** Delete an individual recording from the **Recording** section on its event, or clear everything under **Settings → Meeting Recording → Recordings on This Mac** using **Delete All Audio** or **Delete All Recordings**.

## Troubleshooting {#troubleshooting}

- **System audio missing or “Microphone only”:** Capturing other participants' audio requires **macOS 14.4 or later** and System Audio permission.
- **Microphone blocked:** Enable ThreeStrands under **System Settings → Privacy & Security → Microphone** and restart the recording.
- **Recording stopped early:** Check that your Mac has at least **2 GB** of free disk space and that the session did not pass the 4-hour limit.

See [Calendar](calendar.html#prepare-for-a-meeting), [Privacy](privacy.html#meeting-recording), and [Compatibility](compatibility.html#platforms) for related details.
