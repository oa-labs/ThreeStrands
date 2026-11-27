# Meeting recording and transcription

**Status:** In progress. Stage 0 (the spike) ran in the VM on 2026-11-26; see
[the findings](meeting-recording-spike-findings.md). Its host
measurements are still open. Stage 1 started 2026-11-27. Stages 1 to 3 are
built.
**Baseline:** 0.211.0, reviewed 2026-11-26.
**Builds on:** [Meeting preparation and follow-through](meeting-preparation-design.md)
(stages 1–4 and 5a built) and [Meeting notes](notes-design.md#meeting-notes)
(built). That design put recording and transcription out of its own scope. This
plan adds them as a separate feature that ends in the same follow-through review.
**References:** [anarlog](https://github.com/fastrepl/anarlog) and
[meetily](https://github.com/Zackriya-Solutions/meetily), both Tauri and Rust
meeting recorders under MIT. See [Reference implementations](#reference-implementations).

## Outcome and scope

From a menu bar (tray) icon, or from a meeting in Calendar or Today, the user
starts recording a meeting. ThreeStrands records the microphone and the
computer's audio, transcribes them on the device, and, when the user asks,
drafts a summary with decisions and actions. The user reviews the draft in the
existing **Review Follow-through** dialog and saves it into the meeting's note.

In scope:

- a tray icon owned by the running ThreeStrands process, with recording
  controls and the next meeting;
- capture of microphone and system audio as two separate tracks;
- on-device transcription, with "You" (microphone) and "Others" (system audio)
  as speakers;
- an AI recap (summary, decisions, actions, open questions) with timestamped
  evidence, reviewed before anything is written;
- saving into the occurrence's meeting note through the guarded follow-through
  save, plus an optional transcript file attached to that note;
- local retention and deletion of audio and transcripts.

Out of scope for this plan: starting a recording automatically, joining calls as
a bot, live captions, naming individual remote speakers, playback synced to the
transcript, mobile, and syncing audio or raw transcripts between devices.

## Decisions

| Question | Decision | Why |
| --- | --- | --- |
| Separate app or the existing one? | The tray icon belongs to the ThreeStrands process (Tauri's `tray-icon` feature). There is no second executable. | Notes, the calendar cache, contacts, AI keys, the request journal, and guarded note writes all live in this process and its SQLite database. A second writer would bypass version checks and receipts. |
| What if the window is closed? | While **Meeting Recording** is on, closing the window hides it and the tray icon stays. **Quit ThreeStrands** in the tray menu (or ⌘Q) quits, and asks first while a recording is in progress. | A recording must not stop because a window closed. With the feature off, today's close behavior is unchanged. |
| Who decides when to record? | Only the user. The app can offer ("Record “Weekly Sync”") but never starts recording on its own. | It matches DESIGN.md's "proactive work only prepares proposals" and recording-consent law. |
| Transcription engine | On-device `SpeechAnalyzer` through a small Swift shim on macOS 26+ (default); on-device whisper.cpp (`whisper-rs`) in a killable worker process on macOS 14.4–15, Windows/Linux (stages 6–7), and selectable on macOS 26+. Uploading audio to a cloud provider is a later, opt-in stage. | ThreeStrands is local-first. Audio of other people is the most sensitive data the app would hold. `SpeechAnalyzer` gives Neural Engine efficiency with zero model download on macOS 26+, while whisper.cpp covers earlier OSes and cross-platform builds. |
| Speakers | Two tracks: the microphone is **You** and system audio is **Others**. No diarization in v1. | This gives real speaker attribution for the most important split, "what I committed to" versus "what they did", at no cost. |
| Where results go | The recap goes into the existing `MeetingFollowThroughDialog`, then into a dated, occurrence-linked follow-through section in the meeting note. | That boundary already exists, with receipts, Undo, and conflict handling. AI output stays a proposal. |
| The transcript in notes | Not as note text. A note has a 96 KB limit, and all notes share 3.5 MiB. The transcript can be attached to the meeting note as a Markdown file (attachment path, 20 MiB limit, encrypted sync). This is an option in the review, off by default. | One long meeting would use a large share of the shared notes budget. |
| Audio | Stays on the device. It is never synced or attached, and it is deleted on a retention schedule. | Size, and privacy. |
| Settings transfer | The recording settings are device-local and are not included in the settings export. | Permissions, downloaded models, and audio devices belong to a machine, and keeping them local avoids a change to the export format. |
| Platforms | macOS first (system audio from macOS 14.4, the tap floor), then Windows, then Linux. No recording on Android. | macOS is the primary target, and each OS needs its own system-audio code. |

Settled by the developer on 2026-11-26:

1. **Transcription in v1 is local only.** Cloud upload is the opt-in in stage 8.
2. **macOS below the process-tap floor records the microphone only.** The SDK
   marks process taps as 14.2. meetily's comments say 14.4, and anarlog requires
   15.0. **The floor is macOS 14.4** (the developer, 2026-11-27), so 14.2 and
   14.3 aren't tested. Below it, Settings → Meeting Recording says that system
   audio needs macOS 14.4, and the tray's recording line reads "Microphone
   only". There is no ScreenCaptureKit fallback, so the app never asks for the
   Screen Recording permission. The app's own minimum macOS doesn't change.
3. **Audio is kept 7 days by default.** The **Keep Audio** setting can change
   this.
4. **The tray icon is shown only while Record Meetings is on.** Turning the
   switch off removes the icon, unless a recording or transcription is still
   running. In that case the icon goes once that work ends.

Settled by the developer on 2026-11-27:

5. **System sleep stops an active recording cleanly** and immediately queues
   transcription, so waking a laptop never silently resumes microphone capture.
6. **Apple `SpeechAnalyzer` is the default engine on macOS 26+**, with Whisper
   Fast and Accurate selectable alongside it.
7. **Stage 5 offers cover both scheduled calendar meetings and unscheduled
   calls** from known conferencing apps (Zoom, Slack, Teams, FaceTime, etc.),
   while still never starting or stopping automatically.

## User experience

### Settings → Meeting Recording

A new settings page, off by default:

- **Record Meetings**: the main switch. Turning it on asks for microphone
  permission and system audio permission (on macOS) at that moment, not at
  first launch.
- **Microphone**: defaults to **System Default**, with an optional explicit
  input-device override and the currently active mic shown. This prevents
  macOS from jumping to Continuity Camera or virtual devices, and avoids forcing
  Bluetooth headsets into low-quality HFP mode when the call app uses another
  mic.
- **Transcription Engine / Model**: on macOS 26+, **Apple On-Device**
  (`SpeechAnalyzer`, default, zero download) alongside Whisper **Fast**
  (`small.en` q5_1, 190 MB, English only) and **Accurate** (`large-v3-turbo`
  q5_0, 574 MB, multilingual). On macOS 14.4–15 (and Windows/Linux), Whisper
  **Fast** and **Accurate**. Downloaded Whisper models fetch only when the
  user presses **Download**, with progress, and are checked against a SHA-256
  hash pinned in the policy module. The spike recorded the hashes.
- **Transcription Language**: **Auto** (default; detects the language once over
  the first ~30 s of speech per recording and pins it for the rest of the job)
  or an explicit language choice. Whisper **Fast** (`small.en`) is
  English-only.
- **Transcribe During Meetings**: on by default. Transcribes finished minutes
  in batches while the meeting is recorded, so the transcript is ready soon
  after Stop. Off, nothing is transcribed until Stop, which keeps the Mac's
  memory and processor free during the meeting. See
  [Memory](#memory-during-long-meetings).
- **Keep Audio**: 7 days (default), 30 days, Until Deleted, or Until Transcribed.
- **Offer to Record Meetings**: on by default. Shows the upcoming meeting in the
  tray menu, and adds a tray badge when a known conferencing app (or a browser
  during a scheduled calendar meeting) starts using the microphone, covering
  both scheduled meetings and unscheduled calls.
- **Remind Me to Tell Participants**: on by default. Shows a notice at every
  start.
- **Summarize with AI**: off by default. It uses the user's AI provider and adds
  `meetingRecap` to the per-feature model choice. When it is off, a recording
  ends at the transcript, and the user writes the follow-through themselves.
- **Recordings on This Mac**: used disk space, with **Delete All Audio** and
  **Delete All Recordings**. Both ask for confirmation first.

### The tray icon and menu

The icon is a monochrome template image while idle. While recording it shows a
filled variant, and while transcribing a third variant. Menu items are
title-style. The state lines are disabled items:

```
Record “Weekly Sync”            ← only when a meeting is under way or starts within 10 min
Start Recording                 ← an ad-hoc recording with no meeting
─────────────
Recording “Weekly Sync” · 12:34 ← disabled status line
Pause Recording / Resume Recording
Stop Recording
─────────────
Transcribing “Design Review”… 40%
Review “Design Review”          ← opens the app on the review dialog
─────────────
Open ThreeStrands
Settings…
Quit ThreeStrands
```

The tray menu is a native menu, so its keyboard handling belongs to the OS and
does not touch the command registry. The first **Start** on a device opens the
consent dialog described below. After that, starting takes one step.

### Inside the app

- **Calendar Event Details and Today's Agenda / day-view meeting actions**
  show **Record** next to **Add Note** for a timed meeting that is under way or
  starts within 10 minutes. While that meeting is being recorded, the button
  reads **Stop Recording**. Like other one-step actions on read-only surfaces,
  these are buttons. Space and Enter activate them natively.
- **A recording indicator** in the app chrome (the sidebar, above the account
  switcher) shows a dot, lightweight dual-track activity feedback (mic and
  system audio signal indicators so the user can confirm both tracks are live),
  the elapsed time, and **Stop** while a recording is in progress. If system
  audio delivers only sustained digital zeros while the microphone hears
  speech, it surfaces an in-meeting notice pointing to System Settings. It never
  takes focus. It uses `--text-meta`, `--control-h-sm`, and the shared tokens.
- **The Meeting Preparation view** (shown in Calendar Event Details and Notes'
  context rail) gets a Recording section, in the conversation group after Notes. It has one `ContextRow` per recording of the occurrence.
  The glyph is the audio `AttachmentIcon`. The title is **Recording**, the date
  sits in the row's date slot, and the detail line reads "42 min · Transcribed",
  "Transcribing… 40%", or "Interrupted · 18 min saved". Trailing actions are
  **Review** and **Show Transcript**. The section is left out when there are no
  recordings, so the panel stays short by construction.
- **Command palette**: **Start Recording**, **Stop Recording**, **Pause
  Recording**, **Review Last Recording**. In this release there are no
  single-key shortcuts. `r` (reply) and the others stay as they are, and an
  OS-wide hotkey is left for later. Each new command goes into the central
  registry with collision tests.

### Consent

The first start on a device opens a modal, **Recording Meetings**. It explains
that recording laws differ by place, that participants should be told, and
what is kept and where. Its actions are **Cancel** and **Start Recording**.
**Remind Me to Tell Participants** then shows a short notice at each start:
"Let everyone know this meeting is being recorded." The app never posts to the
meeting chat or the invitation for the user.

### Review

When transcription finishes, the tray offers **Review “…”**, and the Recording
row shows **Review**. Review opens the existing **Review Follow-through** dialog
for the occurrence with:

- **Summary**: a textarea filled from the AI recap, with length scaled to
  meeting duration up to the 400-word cap. With AI off, it starts empty.
  **Include My Notes** can be toggled directly in the dialog (off by default)
  with one-click re-summarizing without re-transcribing. It is saved as a
  nested `Summary:` item under the follow-through line, as ordinary Markdown
  that earlier versions read as plain text.
- **Suggested items**: decisions, tasks, and open questions from the recap. Each
  is marked **Suggested** and shows its evidence: the timestamp, the speaker
  (You or Others — mapped deterministically to the other attendee in a 1:1
  meeting), and the quoted transcript text pulled directly from local SQLite
  segments by `segmentIds` (never trusting LLM-echoed text). The user can edit,
  accept, or remove each item. The existing rules still apply: actions spoken by
  **You** default to round Tasks, actions spoken by Others stay information
  unless the user chooses to track them, and owners and deadlines are never
  invented.
- **Attach Transcript**: a checkbox, off by default and remembered per device.
  When it is checked, saving attaches `Transcript – <meeting> – <date>.md` to
  the destination note and adds a link to it in the section.
- **Destination**: the same occurrence-linked note picker. If no note exists, the
  dialog runs the existing **Add Note** flow first, as it does today.

Saving reuses the existing follow-through save: version-checked, transactional,
receipted, and undoable. Failure keeps both the proposal and the user's edits.

**Ad-hoc recordings** have no event. Their review asks which of today's meetings
the recording belongs to, or **None**. With **None**, the section goes under
`## Meetings` in that day's daily note, with a plain time label and the title
the user typed.

### The transcript view

**Show Transcript** opens a read-only modal (the `modal` scope) with timestamped
lines labelled You or Others, a **Copy** button, and a **Delete Recording**
button that asks first. It has no editor, so the workspace stays read-only.

## Capture

All capture code sits behind a `CaptureBackend` trait so tests can feed it
synthetic PCM.

| Platform | Microphone | System audio |
| --- | --- | --- |
| macOS 14.4 and later | Core Audio directly: an IOProc on the selected input device (or following default input changes when set to System Default), using the same code path as the tap. ThreeStrands' own tap device is left out of the device list. | A Core Audio process tap: a mono `CATapDescription` global tap that excludes ThreeStrands' own process, `AudioHardwareCreateProcessTap`, and a private aggregate device whose tap list holds the tap. Bindings come from `objc2-core-audio` 0.3, in the objc2 family already in the tree. The spike confirmed it exposes everything the tap needs, so `cidre` (used by both references) isn't needed. It needs `NSAudioCaptureUsageDescription` and no Screen Recording permission. |
| macOS before 14.4 | Core Audio, as above | None. The recording has only the microphone track. It also picks up other people's voices from the speakers, so the transcript has no speaker labels, and the recap treats every action's owner as unclear rather than as the user's. |
| Windows | `cpal` (WASAPI) | WASAPI loopback (`cpal` input streams on output devices, as meetily does, or the `wasapi` crate, as anarlog does). The source follows the endpoint another process is actually playing through. It switches only after two polls agree, and skips ThreeStrands' own process. |
| Linux | `cpal` | The PulseAudio monitor source of the default sink through `libpulse-binding`. PipeWire answers it through its Pulse compatibility layer. ALSA is avoided: meetily crashed after about 30 minutes of concurrent ALSA calls. |

Processing:

- Each track is downmixed to mono, resampled to 16 kHz (whisper's input format)
  on a capture thread, and written as 16-bit PCM WAV. On macOS the resampler
  (a windowed-sinc filter) and the WAV writer are written in the app, so
  stage 2 adds only `objc2-core-audio` and `objc2-core-audio-types`, not
  `cpal`, `rubato`, or `hound` (chosen on
  2026-11-27 to keep the unvetted set small; the spike's `cargo vet` found
  35 unvetted crates with them).
  Windows and Linux may still use `cpal` in stages 6 and 7. One resampler
  runs per stream for the whole recording. meetily found that
  making a new one per chunk raised the volume by about 173%.
- **Tracks are never mixed.** Both references mix mic and system audio into
  one stream at some point. Keeping them apart is what gives the You/Others
  labels.
- **The aggregate device holds only the tap.** Adding the output device as a
  sub-device as well recorded the audio twice (meetily). The tap is mono, so
  its stream is opened with one channel.
- **Stalls.** A source that delivers no frames for 5 seconds is restarted, with
  backoff at 1, 2, and 4 seconds (anarlog). One failed track does not stop the
  other. Device changes come from Core Audio's default-device and device-list
  listeners, debounced, rather than from polling.
- **Writes** are flushed to disk about once a second.
- **Aligning the tracks.** The tap delivers nothing until some app first plays
  audio; in the spike, the first callback came 5 seconds after the tap
  started. So each track stores the host time of its first callback
  (`AudioTimeStamp.mHostTime`), and the merge places it on the recording's
  clock from that time. Counting frames from Start is not enough.
- **The IOProc** writes into a preallocated ring buffer and never allocates.
- **Segment files and post-transcription transcoding.** During capture, each
  track is written in 60-second 16 kHz mono 16-bit PCM WAV files whose headers
  are finalized when the file closes. A crash or power loss costs at most the
  open minute. On the next launch, an unfinished recording is marked
  **Interrupted**, and its saved minutes can still be transcribed. Live WAV
  segments take about 2 MB per minute per track (~230 MB for a one-hour,
  two-track call). Once transcription completes — when **Keep Audio** is not
  `Until Transcribed` — retained tracks are transcoded to AAC/M4A through
  macOS's native `AudioToolbox` (`ExtAudioFile`, zero new crates; ~7–8× smaller,
  ~25–35 MB/hr) and the raw WAV segments are removed. Retrying transcription
  decodes the M4A back to 16 kHz PCM via `ExtAudioFile`.
- **Pause stops hardware.** Pausing ends the current segment, records a gap, and
  stops the Core Audio IOProc streams (`AudioDeviceStop`) rather than merely
  dropping frames, so macOS turns off the orange microphone indicator and
  releases Bluetooth HFP until Resume.
- **Gaps and sleep.** A pause or device change ends the current segment and
  records a gap, so timestamps stay true to the wall clock. A change of the
  default input device moves to the new one when set to System Default.
  **System sleep** (`NSWorkspaceWillSleepNotification` on macOS) stops the
  active recording cleanly and immediately queues transcription, rather than
  resuming microphone capture on wake.
- **Silent-tap permission detection.** On macOS 15+, denied or revoked system
  audio permission causes the tap to stream exact digital zeros. If the tap
  delivers sustained exact zeros for ~15–20 seconds while the microphone hears
  speech, the app shows an in-meeting warning deep-linking to
  `x-apple.systempreferences:com.apple.preference.security?Privacy_AudioCapture`.
- **File permissions and backup exclusion.** On Unix/macOS, recording audio
  directories are created `0700` and audio files `0600`. Both the audio
  directory and `recording-models/` are marked with
  `NSURLIsExcludedFromBackupKey` so Time Machine and backup tools do not
  archive sensitive audio off-device.
- **Limits** (all in `src-tauri/src/recording/policy.rs`): at most 4 hours per recording,
  recording stops with a notice when free disk space falls below 2 GB, and only
  one recording runs at a time.
- **macOS packaging.** Add `NSMicrophoneUsageDescription` and
  `NSAudioCaptureUsageDescription` to `src-tauri/Info.plist`, and an
  entitlements file with `com.apple.security.device.audio-input` for the
  hardened runtime.

## Transcription

- **Engines.** Two first-class on-device backends sit behind `SpeechEngine`,
  feeding the same timestamped-line, merge, and storage pipeline:
  - **Apple `SpeechAnalyzer` (macOS 26+, default on macOS 26+):** runs
    on-device through a small Swift shim with Neural Engine acceleration and
    zero model download.
  - **whisper.cpp through `whisper-rs` 0.16 with Metal:** default on macOS
    14.4–15, selectable (**Fast** or **Accurate**) on macOS 26+, and planned
    for Windows/Linux in stages 6 and 7 (leaving non-macOS builds free of cmake
    and libclang until then). It runs in a child process, re-executing the app
    binary with a hidden argument and an environment marker as `pdf_worker.rs`
    does, so a C++ crash or runaway memory use can't take down the mail client.
    **A worker per batch**: a worker loads the model, transcribes the segment
    files waiting, and exits, so the model is in memory only while there is
    work for it (see [Memory](#memory-during-long-meetings)). After Stop one
    worker does the rest of the recording. The worker gets an empty environment,
    `/` as its directory, no core dumps, and nice 10. The parent enforces a
    load timeout, a per-request wall-clock timeout scaled to the request's
    audio, the model's memory budget (its physical footprint, polled, as macOS
    doesn't enforce `RLIMIT_AS`), and a stop check (cancel, memory pressure),
    and kills and reaps the worker on any breach, on malformed output, and on
    drop. **Protocol**: length-prefixed JSON frames both ways. A `config` frame
    first (model paths, language, initial prompt, threads), answered by
    `ready`; then one `segment` request at a time (track, WAV path, `startMs`,
    whether it continues the previous file of the track's run, whether it ends
    the run, and, on a worker's first file of a track, the `carryIn` an earlier
    worker held back), each answered by its `lines` (`track`, `startMs`,
    `endMs`, `text`, and per-segment RMS energy) and `held`: where the audio the
    worker still holds for that track starts, as files and sample offsets on
    the recording's clock. The parent checks every path: the exact file name
    the segment writer gives, resolved inside the recording's audio folder,
    including every path a worker reports holding.
- **Vocabulary seeding and language pinning (Whisper).** `initial_prompt` is
  seeded with a short, sanitized vocabulary hint (~50–80 tokens max) built from
  the calendar event title and attendee display names, applied only to spans
  ≥ ~1 s (with exact prompt-echo filtering) to improve proper-noun spelling
  without hallucinations. Transcription follows the **Transcription Language**
  setting: under **Auto**, language is detected once over the first ~30 s of
  speech per recording and pinned for the remainder of the job (avoiding
  per-span language drift under `set_no_context(true)`). Whisper **Fast**
  (`small.en`) always passes `"en"`.
- **Speech across segment files.** Speech that reaches the end of a segment
  file is held back with the last half second of every file and transcribed
  with the next contiguous file of the same run, so a sentence is never cut at
  a minute boundary; the run's last file flushes it. What is held never
  exceeds one 30-second span. **Across workers too**: when a worker exits
  between batches, the next worker's first file of the track carries the
  held audio in (`carryIn`), read from the same files and offsets, so the
  next worker transcribes exactly the samples, on exactly the clock, that one
  long-lived worker would have. The transcript and the percentages are the
  same either way, which a test checks by transcribing the same audio both
  ways.
- **Builds.** `.cargo/config.toml` (at the root and in `src-tauri`) sets
  `GGML_NATIVE=OFF`, so whisper.cpp is compiled for the architecture's
  baseline, not the build machine's CPU, and `CMAKE_OSX_DEPLOYMENT_TARGET=10.15`,
  because whisper.cpp uses `std::filesystem`, which needs macOS 10.15. That is
  also the app's minimum (`minimumSystemVersion` in `tauri.conf.json`, raised
  from Tauri's default 10.13 on 2026-11-27); the setting covers cargo builds
  that Tauri doesn't start.
- **Voice activity detection** runs first, using Silero, so silence produces
  no "Thank you." hallucinations and long quiet stretches cost nothing. It runs
  as a separate step: `WhisperVadContext`, then whisper on each speech span.
  whisper-rs's `FullParams::enable_vad` is silently ignored, because
  `WhisperState::full` calls `whisper_full_with_state`, which skips VAD. In
  the spike, Silero took 0.12 s for 94 s of audio. Start from meetily's
  tuning:
  - speech thresholds 0.50 to start and 0.35 to continue;
  - 300 ms before and 400 ms after speech;
  - a minimum speech length, because whisper rejects 40 ms fragments;
  - a maximum segment length (about 30 s, set in the policy module), because
    continuous system audio otherwise held a segment open indefinitely.

  whisper.cpp derives the continue threshold as the start threshold minus
  0.15, and pads both sides equally, so the app sets the start threshold and
  adds the asymmetric padding itself.
- **Timestamp tokens stay on.** This departs from meetily. Each span is 30
  seconds or shorter, where the tokens drop nothing: the spike measured 1.9%
  WER with them on or off. They also split spans into sentences, which the
  echo merge needs. meetily's dropped segments happen on longer inputs: 44%
  WER without VAD and tokens off. Segment times are the span's start plus
  whisper's offsets. Flash attention is off on macOS, where it crashes
  (anarlog). A post-filter trims lines and removes empty ones, consecutive
  repeats within a track, known hallucinations (a list in the policy module),
  and bracketed non-speech such as "(music)".
- **During the recording.** With **Transcribe During Meetings** on (the
  default), finished minute-segments are transcribed in batches while
  recording continues, so the transcript is nearly ready when the user
  presses Stop: once 5 minutes of finished audio wait (`LIVE_BATCH`), a
  worker starts, transcribes them, and exits. On battery, this waits until
  Stop (IOKit's providing power source), and a batch in progress ends after
  its current file. Turned off, nothing is transcribed until Stop, and a
  batch in progress stops at once. One job runs at a time: waiting
  `recorded` and `interrupted` recordings go first, oldest first, whenever
  the chosen model is ready. Nothing of the work during a recording is
  visible until Stop; then the row shows `transcribing` with a percentage
  that counts what was already done. A job keeps the model it started with
  and checks its hash before every worker loads it. A job that fails during
  the recording starts over after Stop.
- **Merging tracks.** Segments are ordered by start time. When every output is
  headphones, the microphone hears only the user, so nothing need be dropped.
  anarlog uses the same headphones test to switch its echo canceller off. On
  macOS, output-route detection checks `kAudioDevicePropertyTransportType`
  (Bluetooth/USB) and headphone-jack data source (`'hdpn'`) to bypass or tighten
  echo dropping when no speaker-to-mic path exists (initially deferred in stage 3
  until host verification). Otherwise the microphone picks up the room's
  speakers. The merge works sentence by sentence, using a **causal
  (asymmetric) time window** `[system.start - 250 ms, system.end + 1000 ms]`
  rather than symmetric `±1 s` so a remote speaker repeating what the user just
  said does not falsely drop the user's line. A microphone sentence is dropped
  when at least half of its non-stop-words (0.5, in the policy module, above a
  minimum content-word count so short user confirmations like "Yeah, on Friday"
  are protected) also appear in overlapping system sentences, paired with a
  per-segment RMS-energy ratio check (computed directly in Rust, no ML runtime).
  In the spike's simulated echo (−16 dB, 60 ms), word overlap dropped all 13
  echo sentences (scores 0.70–1.00) while keeping the user's line spoken over
  the remote speaker (0.25) and the user's line alone (0.00). Comparing whole
  30-second spans instead lost the line spoken over the remote speaker. This is
  pure, testable code. Two kinds of echo cancellation are ruled out for v1:
  - Apple's voice-processing I/O, because it lowers all other audio by about
    8 dB, the call included, with no public way to stop it (anarlog measured
    this).
  - A neural echo canceller like anarlog's (ONNX models run through `ort`),
    which is the next step only if real speaker calls still measure too much
    duplication.
- **The model** is stored under the app data directory, in
  `recording-models/`. Downloads start from the pinned Hugging Face URL and
  follow at most five redirects, each over https to Hugging Face's own hosts.
  They resume after an interruption with HTTP Range requests (starting over
  when the server ignores the range), stop past the pinned size, and refuse
  to start when the disk would fall below the 2 GB recording floor. The voice
  activity model downloads first, as part of either model's download. The
  model is checked against a pinned SHA-256 after download and before every
  load; a damaged file is deleted and the model shown as failed. That is
  stricter than either reference: meetily checks only the size, and anarlog a
  CRC32.

### Memory during long meetings

Other meeting tools weigh a Mac down after half an hour or so, apparently
pushing it into swap. Nothing here grows with the length of a meeting, and
the model is in memory only while it has work:

- **Capture** holds fixed-size rings and buffers: 2-second rings between the
  device callback and the capture thread, one resampler per stream whose
  buffer is trimmed after each block, and a segment writer that streams each
  60-second file to disk. Tests push two hours of synthetic audio through the
  rings, the downmix, the resampler, the segment writer, and the service as
  fast as the machine allows, counting heap bytes: what is held after two
  hours is what was held after ten minutes (no growth; a peak of about
  17 KB above the first minute's). The windowed-sinc filter, the slow part
  of a debug build, gets its own 20-minute run at 8 kHz, with the same
  result.
- **The parent side of a job** keeps, per track, only the number of the last
  file done and the audio held back, plus the transcript lines themselves.
  Each step reads the manifests a line at a time rather than listing every
  file. A test runs a two-hour job (batches, the merge, and the save) with a
  fake transcriber returning about 150 words a minute: beyond the transcript
  itself, a step near two hours holds and peaks at what one near ten
  minutes does (the peak is the 1 MB buffer that checks the model's hash).
- **Workers come and go.** During the recording the model is loaded for a
  batch (5 minutes of audio, a minute or two of work) and released, rather
  than held for the whole meeting. After Stop one worker does the rest.
- **Memory pressure.** The scheduler reads macOS's memory pressure level
  (`kern.memorystatus_vm_pressure_level`, the levels a
  `DISPATCH_SOURCE_TYPE_MEMORYPRESSURE` source reports: normal, warning,
  critical) behind a `MemoryProbe` trait, polled while a worker runs and
  before one starts. No worker starts while pressure is warning or
  critical. During the recording, warning or critical stops the batch's
  worker at once; the file it was on is redone later, from the last one it
  finished, and nothing more is transcribed until Stop. After Stop, only
  critical stops a running job; the recording goes back to `recorded`, and
  the job goes on from where it stopped when pressure is normal again.
- **Available memory.** Before a worker starts, the free, inactive,
  purgeable, and speculative pages (`host_statistics64`, as `vm_stat`
  reports them) must cover the model's budget plus 512 MB
  (`WORKER_START_MARGIN_BYTES`). Until then the job waits and its recording
  stays `recorded`.
- **Per-model budgets.** A worker is killed, failing the job with a plain
  message, when its physical footprint passes its model's budget:
  Fast 1,150 MiB and Accurate 1,900 MiB (`WORKER_MEMORY_FAST_BYTES`,
  `WORKER_MEMORY_ACCURATE_BYTES`; the flat 3 GiB cap before them is gone). Each is 1.5 times the peak measured on Metal,
  but no lower than 1.5 times the spike's CPU peak, which is the larger for
  both models in the VM.

Measured in the VM (2026-11-27, a release build of the worker), the real worker in batches over
speech-like audio built from `say` with pauses and noise, its physical
footprint sampled every 10 seconds:

| Model | Audio | Wall time | Footprint per quarter of the run (min / median / max, MiB) | Peak |
| --- | --- | --- | --- | --- |
| Fast | 120 min | 48 min | 682 / 685 / 700; 524 / 685 / 685; 524 / 685 / 686; 525 / 683 / 686 | 700 MiB |
| Accurate | 20 min | 29 min | 1,069 / 1,146 / 1,146; 1,140 / 1,149 / 1,149; 1,003 / 1,149 / 1,152; 1,145 / 1,146 / 1,149 | 1,152 MiB |

The test fed each 5 minutes of audio at once, so batches ran back to back
and a worker was running at nearly every sample; in a real meeting a Fast
worker runs for about 2 of every 5 minutes. The footprint has no upward
trend: each batch's worker starts again at the same size, and the last
quarter's maximum is at or below the first's. The VM's Metal is slow (real
time factors of about 0.4 for Fast and 1.4 for Accurate here, beam search
5), so the wall times say little about real Macs.

The VM's GPU is Apple's paravirtual device, not real Apple Silicon, so Metal
there allocates differently. The developer's Mac must confirm the budgets:
run the ignored `real_worker_memory_stays_flat_over_a_two_hour_recording`
test (instructions in its comment) for each model and check that the peak
stays well under the budget.

## AI recap

- A new `Feature::MeetingRecap` (`meeting_recap`), an `AiModelUse`
  `meetingRecap` defaulting to the reasoning model, and a module
  `src-tauri/src/ai/meeting_recap.rs`. It follows `ai/meeting.rs`: a system
  prompt that treats the transcript as untrusted evidence, structured output,
  `with_purpose` usage logging, request controls, and the daily spending limit.
- **Input**: the merged transcript as numbered segments
  (`{id, speaker: "you"|"others", start, text}`), plus the reviewed event title,
  time, and attendee display names. In a **1:1 meeting** (exactly one other
  attendee), `others` maps deterministically to that attendee; in multi-attendee
  meetings, the prompt instructs the model never to guess which remote attendee
  spoke an `others` line unless their name is spoken in the transcript.
  **Include My Notes**, off by default and toggled directly in the review dialog
  (with one-click re-summarizing without re-transcribing), adds what the user
  typed in this occurrence's note excerpt during the meeting and instructs the
  model to treat those jotted notes as priority focus/structure anchors while
  grounding all items in `segmentIds`. A note marked **Exclude from AI** is
  never offered. Nothing else is sent: no mail and no contact notes. The
  summary's length guidance scales with the meeting's duration (~60–100 words
  for a 15-min standup, ~200–350 for a 60-min meeting).
- **Output schema**: `summary` (400 words at most), `decisions[]`, `actions[]`
  (`owner: you|attendee|unclear`), and `openQuestions[]`, each with non-empty
  `segmentIds`. Validation checks that every cited ID exists, and displayed
  evidence quotes are pulled directly from local SQLite segments by
  `segmentIds` rather than trusting LLM-echoed quote text. An item that fails
  validation is withheld and counted ("2 suggestions withheld"), never shown as
  an empty result.
- **Limits.** A transcript-bytes limit for one request lives in the policy
  module. Choose it in stage 1 from current provider context sizes; about
  400 KB, a long three-hour meeting, is a starting point. Above the limit, the
  user picks a time range. Text is never silently truncated, and costly work
  is never automatically split into batches. The 150-second execution limit
  in [AI requests](ai-request-foundations.md) may need its own allowance for
  this purpose; measure it before raising anything.
- **When it runs**: when the user presses **Summarize** in the review, or
  automatically after transcription only if **Summarize with AI** is on. In
  that case it only prepares a proposal, as proactive suggestions do.

## Storage, retention, and compatibility

- **Migration** (the next number): `meeting_recordings` (`id`, `event_ref` JSON
  or NULL, `title`, `started_at`, `ended_at`, `duration_ms`, `state`
  recording|interrupted|recorded|transcribing|transcribed|failed, `engine`, `model`,
  `audio_dir`, `audio_deleted_at`, `recap_json` NULL, `reviewed_operation_id`
  NULL, `error`), where `recorded` means stopped and waiting for
  transcription, and `meeting_recording_segments` (`recording_id`, `seq`,
  `track`, `start_ms`, `end_ms`, `text`, `rms`, `dropped_as_echo`).
- **Settings** live natively in a single-row `meeting_recording_settings`
  table, because the tray, retention, and transcription read them while the
  window may be hidden. The webview reads and writes them through
  `recording_settings` and `set_recording_settings`. Fields added later
  (`microphone_device_uid`, `transcription_engine`, `transcription_language`)
  get serde defaults.
- **Device-local.** These tables are not replicated entities and not part of
  settings transfer. Audio files are owner-only (`0700`/`0600`) and excluded from
  backups (`NSURLIsExcludedFromBackupKey`), and completed recordings are
  transcoded post-transcription to AAC/M4A unless **Keep Audio** is
  `Until Transcribed`. The only things that leave the device are what the user
  saves into a note and the optional transcript attachment, both through
  existing sync paths. No sync format change.
- **Retention sweep** at launch and daily: delete audio past **Keep Audio**.
  Transcripts stay until the user deletes the recording. Deleting a recording
  never touches notes already saved from it.
- **The review is reviewed** when a follow-through section linked to the
  occurrence exists, as defined in meeting preparation. The recording row also
  keeps the local operation ID of the save.

## Privacy and security

- Only the user starts a recording, and recording is always visible in the tray
  icon, the in-app indicator, and the OS microphone indicator.
- System sleep stops an active recording cleanly and queues transcription, so
  waking a laptop never silently resumes microphone capture.
- Pausing stops the audio hardware streams so the OS microphone indicator turns
  off during off-the-record sidebars.
- Audio and transcripts stay on the device. Audio directories and files are
  created with owner-only permissions (`0700`/`0600`) and excluded from backups
  (`NSURLIsExcludedFromBackupKey`), along with `recording-models/`. Nothing
  reaches an AI provider until the user turns on **Summarize with AI**, and then
  only the transcript text and the reviewed event fields are sent.
- Transcript text is untrusted. It is rendered as text, displayed evidence
  quotes are pulled directly from local SQLite by validated `segmentIds`, and
  instructions spoken in a meeting cannot authorize writes or tools.
- Model downloads use the app's HTTP client, pinned hashes, and a fixed host.
  Nothing is executed from the download.
- The worker gets an empty environment, resource limits, a wall-clock timeout
  scaled to audio length, and is killed on drop, as the PDF worker is.

## Native layout and commands

```
src-tauri/src/
  tray.rs                      tray icon, menu, state → menu rebuild, window hide/show
  recording/
    mod.rs                     RecordingService: state machine, one active session
    policy.rs                  all limits, thresholds, model hashes and sizes
    capture/{mod,macos,windows,linux,fake}.rs
    segments.rs                WAV segment writer, gap markers, crash recovery
    transcribe/                models, worker + protocol, stream + spans (in the
                               worker), engine (SpeechAnalyzer Swift shim +
                               whisper on macOS), plan, merge, scheduler, power,
                               transcode (AudioToolbox M4A/AAC)
    retention.rs
  ai/meeting_recap.rs
  commands/recordings.rs
  db/recordings.rs
```

Commands: `recording_start(eventRef?, title?)`, `recording_pause`,
`recording_resume`, `recording_stop`, `recording_status`,
`list_recordings(eventRef)`, `recording_transcript(id)`,
`delete_recording(id)`, `delete_recording_audio(id)`,
`recording_settings`, `set_recording_settings(settings)`,
`transcription_models`, `download_transcription_model(model)`,
`cancel_transcription_model_download(model)`,
`delete_transcription_model(model)`, `transcribe_recording(id)` (Retry), and
`summarize_recording(id, requestId, range?)` (stage 4). Events: `recording-state`
(the indicator), `recordings-changed` (the Recording rows, including every
transcription state change), `transcription-progress` (whole-percent changes),
and `transcription-models-changed` (at most four a second while downloading).
The webview side adds domain types in `src/domain.ts`, the client in
`src/data/client.ts`, and fixtures in `src/data/demoClient.ts`.

## Delivery stages

Each stage works without the ones after it. Add a user-facing changelog entry
in the first stage users can see (stage 2), and extend it in later stages.

0. **Spike.** *Ran 2026-11-26 in the VM.* See the
   [findings](meeting-recording-spike-findings.md) and the throwaway
   crate in `spikes/meeting-recording/`. Still open: the host run (a real call
   on speakers and on headphones, Metal speed, the permission prompt) and
   confirming the tap records on macOS 14.4, the floor. Parakeet is deferred. The original scope: record a call
   through a macOS process tap and the microphone to WAV. Build `whisper-rs` with
   Metal in CI. Measure realtime factor, memory, battery, and echo duplication
   with and without headphones. Also settle three questions:
   - the earliest macOS on which a tap actually records;
   - whether `objc2-core-audio` exposes the tap API, or `cidre` is needed;
   - whether Parakeet TDT (ONNX through `ort`, meetily's default engine) beats
     whisper's speed and accuracy enough to justify a second runtime.

   Write the findings to `docs/meeting-recording-spike-findings.md`. Gate:
   the crate choices are confirmed, and `pnpm supply-chain` results for
   `tray-icon`, `cpal`, `whisper-rs` and its build dependencies, `rubato`, and
   `objc2-core-audio` are reported to the developer. They are not certified by
   an agent.
1. **Contracts.** The policy module, the migration, the domain and demo types,
   the device-local settings, `Feature::MeetingRecap`, and the recording state
   machine against the fake backend. No UI.
2. **Record on macOS.** Tray icon and menu, window hide while the feature is
   on, the quit confirmation, permissions and entitlements, the consent modal,
   the in-app indicator, **Record** on Event Details and Today, the Recording
   section, palette commands, crash recovery, and retention.
3. **Transcribe.** Model download and verification, the worker, transcription
   during the recording, the track merge with echo drop, and **Show
   Transcript**. *Built 2026-11-27*; the headphones exemption was initially
   deferred (see Merging tracks). Smoke-tested in the VM with the Fast model on
   Metal: RTF about 0.33 on the paravirtual GPU, model load 4.6 s cold and 0.15 s
   warm. Real Apple Silicon speed and memory are still to measure on the host.
3b. **Hardening & macOS polish.** Refinements to built Stages 2–3:
   - sleep-to-stop (`NSWorkspaceWillSleepNotification`) and hardware-stop on Pause;
   - `0700`/`0600` audio permissions and `NSURLIsExcludedFromBackupKey` on audio
     and `recording-models/`;
   - **Microphone** selector (System Default vs explicit device) and dual-track
     activity feedback in the indicator;
   - faster 15–20 s digital-zero silent-tap warning with System Settings deep
     link (`Privacy_AudioCapture`);
   - causal echo window (`-250 ms` to `+1000 ms`), stop-word / short-confirmation
     protection, headphones output-route detection, and per-segment RMS check;
   - Whisper `initial_prompt` vocabulary seeding and **Transcription Language**
     setting (detect-once Auto);
   - Apple `SpeechAnalyzer` Swift shim as the first-class default engine on
     macOS 26+;
   - post-transcription `AudioToolbox` (`ExtAudioFile`) AAC/M4A transcoding for
     retained audio (with decode-on-retry).
4. **Recap and review.** `meeting_recap.rs`, validation, duration-scaled
   400-word Summary field, 1:1 deterministic naming and group non-guessing,
   SQLite-sourced evidence quotes, Granola-style `Include My Notes` anchoring
   with one-click re-summarize in `MeetingFollowThroughDialog`, **Attach
   Transcript**, and ad-hoc destinations.
5. **Offers.** "Record “…”" for the meeting under way, and the badge when a
   known conferencing app (Zoom, Slack, Teams, FaceTime, Webex, Discord, etc.)
   — or a browser during a scheduled calendar meeting — starts using the
   microphone, covering both scheduled meetings and unscheduled calls. On macOS
   this works with:
   - a listener on the default input's `kAudioDevicePropertyDeviceIsRunningSomewhere`;
   - Core Audio's process list, matched against a curated conferencing bundle-ID
     allowlist and a dictation/system-tool denylist (Siri, Dictation, Wispr
     Flow, Voice Memos, etc.), always excluding ThreeStrands' own PID;
   - a short delay before the offer, so a brief microphone check doesn't
     trigger it.

   On Windows the offer comes from WASAPI session enumeration. When the call
   app releases the microphone, the tray offers **Stop Recording** and never
   stops on its own (anarlog's documentation warns that automatic stop can fire
   early). If the call app released the microphone *and* both tracks stay
   silent for ~3–5 minutes (or past the event's end time), show a tray/app
   "Still recording — Stop?" nudge.
6. **Windows.** WASAPI loopback, tray, and installer permissions.
7. **Linux.** The monitor source and the tray (AppIndicator), with
   `docs/linux-builds.md` updated.
8. **Optional: cloud transcription.** OpenAI-compatible
   `/audio/transcriptions` for providers that offer it, with per-minute prices
   in the spending limit, behind its own switch and disclosure. Handle the
   ~25 MB upload limit by compressing to M4A/Opus or VAD-chunking **per track**
   (never mixing tracks before upload), with optional remote-track diarization
   for providers that support it.

## Reference implementations

anarlog (Fastrepl) and meetily (Zackriya Solutions) are MIT licensed, except
anarlog's `enterprise/` directory, which is under a commercial license and must
not be read or copied. Adapted code keeps its MIT copyright notice in the file
header and in the bundle's third-party notices. Read them for guidance first;
copy only small, self-contained pieces.

| Concern | Where to look | What this plan takes |
| --- | --- | --- |
| Process tap | meetily `frontend/src-tauri/src/audio/capture/core_audio.rs`; anarlog `crates/audio-actual/src/speaker/macos.rs` and `tools/meeting-capture/Sources/CaptureCore/CallAudio.swift` | A mono tap, an aggregate device that holds only the tap, and excluding its own process |
| Device recovery | anarlog `crates/device-monitor`, `crates/audio-actual/src/capture/recovery.rs` | Listeners instead of polling, restarting a stalled source with backoff, and tracks that fail independently |
| Windows and Linux | anarlog `speaker/windows.rs`, `speaker/linux.rs`; meetily `audio/capture/pulse_linux.rs` | Following the active endpoint, and PulseAudio monitors |
| VAD and whisper tuning | meetily `audio/vad.rs`, `audio/pipeline.rs`, `whisper_engine/whisper_engine.rs` | Thresholds, padding, minimum and maximum segment length, and the hallucination filter |
| Crash recovery | anarlog `listener-core/src/actors/recorder/chunks.rs`; meetily `audio/incremental_saver.rs` | Minute segments flushed every second, and recovery at startup |
| Meeting detection | anarlog `crates/detect`, `docs/automatic-capture.mdx` | Offering only; never starting or stopping on its own |
| Tray | anarlog `plugins/tray`; meetily `src/tray.rs` | Tauri `TrayIconBuilder` in the main process, template icons, and a state-driven menu |

Where this plan differs on purpose:

- **Separate tracks, not a mix.** meetily mixes the tracks; this plan never does.
- **whisper in a worker process.** Both references run whisper inside the app.
  anarlog switched off flash attention after it crashed on macOS, and a crash
  like that would close the mail client.
- **A pinned SHA-256 for models.** The references check only size or CRC32.
- **No local LLM.** Summaries use the user's existing AI provider. meetily's
  `llama-helper` sidecar and anarlog's Ollama and LM Studio support are out of
  scope.

## Testing

Following AGENTS.md, at the lowest level that owns each behavior:

- **Native**: the state machine with the fake backend (start, pause, resume,
  stop, double start, stop while transcribing, quit while recording, sleep-to-stop);
  hardware-stop on pause; `0700`/`0600` permissions and backup exclusion;
  segment writer recovery from a truncated final file; gap timestamps; policy
  limits below, at, and above each one (length, disk floor, transcript bytes,
  400-word summary cap); model hash rejection; the worker protocol with
  malformed frames, timeout, and kill; `initial_prompt` cap/sanitization and
  detect-once language pinning; M4A/AAC transcode and decode-on-retry
  round-trip; merge ordering, causal echo window, short-confirmation retention,
  and RMS-assisted echo drop at the similarity threshold; recap schema
  validation, withheld counts, unknown segment IDs, 1:1 deterministic naming vs
  group non-guessing, and SQLite-sourced evidence quotes; prompt-injection text
  in a transcript; conferencing allowlist/denylist filtering and post-call
  silence nudge; retention sweep; migration; heap use that doesn't grow over two
  hours of capture or of a transcription job (counted by a test allocator);
  batches matching one worker's transcript; memory pressure and available
  memory through a fake `MemoryProbe`.
- **Frontend**: the consent and review modals (initial focus, Tab trap,
  Escape restoring focus, background inert, edits kept on failure, Cmd/Ctrl+Enter
  submit); the Recording section rows and their omission when empty; the
  indicator never taking focus; no editable control in the event panel; no
  note write before submission; and title-style labels.
- **App wiring (slim)**: palette commands reaching the service, **Record**
  reaching `recording_start` with the occurrence reference, and the tray's
  **Review** opening the dialog.
- **Developer-verified**: real capture on each OS, permission prompts, tray
  rendering, and echo behavior on speakers versus headphones.

## Risks

- **Consent law.** Two-party-consent places. Mitigated by user-only starts,
  visible state, the consent dialog, and the per-start reminder; the
  responsibility stays with the user.
- **Build and supply chain.** Apple `SpeechAnalyzer` is now the first-class
  default on macOS 26+ (zero download, Neural Engine), while whisper.cpp remains
  required for macOS 14.4–15, Windows, and Linux and brings a C/C++ build
  (cmake), Metal shaders, and a larger binary. If vetting or CI cost for
  `whisper-rs` is unacceptable on non-26 targets, the fallback is the stage 8
  cloud path.
- **Echo and quality** without headphones. Measured in the spike; the causal
  merge rule, short-confirmation guard, RMS check, and headphones route
  detection are the first defense.
- **Bluetooth headsets.** Recording from a headset's microphone forces it
  into its low-quality call mode for the whole stream. Some headsets also cut
  the microphone between words. The explicit **Microphone** selector lets users
  pin their Mac/USB mic while listening on AirPods, and Settings notes this
  when the active input is a Bluetooth device. Both references learned this the
  hard way.
- **System audio permission has no public check.** anarlog asks a private TCC
  framework, which this plan avoids. On macOS 15+, recurring TCC consent or
  denied/revoked permission streams exact digital zeros. If the tap delivers
  only digital zeros for ~15–20 seconds while the microphone hears speech, the
  app shows a warning deep-linking to
  `x-apple.systempreferences:com.apple.preference.security?Privacy_AudioCapture`.
- **Resource use** during long calls. Memory doesn't grow with a meeting's
  length (measured, see [Memory](#memory-during-long-meetings)); the model
  is in memory only for each 5-minute batch; no worker starts under memory
  pressure or without the model's budget free, and pressure stops one that
  is running. Transcription waits until Stop on battery or with
  **Transcribe During Meetings** off, and the worker runs at low priority.
  The per-model budgets were measured in the VM and still need the
  developer's Mac to confirm them.
- **macOS API surface.** Process taps are recent and lightly documented, and
  hardened-runtime entitlements must be right before notarization.
