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
| Transcription engine | On-device whisper.cpp (through `whisper-rs`) in a killable worker process. Uploading audio to a cloud provider is a later, opt-in stage. | ThreeStrands is local-first. Audio of other people is the most sensitive data the app would hold. |
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

## User experience

### Settings → Meeting Recording

A new settings page, off by default:

- **Record Meetings**: the main switch. Turning it on asks for microphone
  permission and system audio permission (on macOS) at that moment, not at
  first launch.
- **Transcription Model**: **Fast** (`small.en` q5_1, 190 MB, English only)
  or **Accurate** (`large-v3-turbo` q5_0, 574 MB, multilingual), with
  **Download** and progress. The model is downloaded only when the user
  presses **Download**, and its SHA-256 is checked against a hash pinned in
  the policy module. The spike recorded the hashes.
- **Keep Audio**: 7 days (default), 30 days, Until Deleted, or Until Transcribed.
- **Offer to Record Meetings**: on by default. Shows the upcoming meeting in the
  tray menu, and adds a tray badge when another app starts using the microphone
  during a calendar meeting.
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
  switcher) shows a dot, the elapsed time, and **Stop** while a recording is in
  progress. It never takes focus. It uses `--text-meta`, `--control-h-sm`, and
  the shared tokens.
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

- **Summary**: a textarea filled from the AI recap. With AI off, it starts
  empty. It is saved as a nested `Summary:` item under the follow-through line,
  as ordinary Markdown that earlier versions read as plain text.
- **Suggested items**: decisions, tasks, and open questions from the recap. Each
  is marked **Suggested** and shows its evidence: the timestamp, the speaker
  (You or Others), and the quoted transcript text. The user can edit, accept,
  or remove each item. The existing rules still apply: actions spoken by
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
| macOS 14.4 and later | Core Audio directly: an IOProc on the default input device, following device changes, the same code path as the tap. ThreeStrands' own tap device is left out of the device list. | A Core Audio process tap: a mono `CATapDescription` global tap that excludes ThreeStrands' own process, `AudioHardwareCreateProcessTap`, and a private aggregate device whose tap list holds the tap. Bindings come from `objc2-core-audio` 0.3, in the objc2 family already in the tree. The spike confirmed it exposes everything the tap needs, so `cidre` (used by both references) isn't needed. It needs `NSAudioCaptureUsageDescription` and no Screen Recording permission. |
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
- **Segment files.** Each track is written in 60-second files whose headers are
  finalized when the file closes. A crash or power loss costs at most the open
  minute. On the next launch, an unfinished recording is marked **Interrupted**,
  and its saved minutes can still be transcribed. That is about 2 MB per minute
  per track, so a one-hour, two-track recording takes about 230 MB until
  retention removes it.
- **Gaps.** Sleep, a pause, or a device change ends the current segment and
  records a gap, so timestamps stay true to the wall clock. A change of the
  default input device moves to the new one.
- **Limits** (all in `src-tauri/src/recording/policy.rs`): at most 4 hours per recording,
  recording stops with a notice when free disk space falls below 2 GB, and only
  one recording runs at a time.
- **macOS packaging.** Add `NSMicrophoneUsageDescription` and
  `NSAudioCaptureUsageDescription` to `src-tauri/Info.plist`, and an
  entitlements file with `com.apple.security.device.audio-input` for the
  hardened runtime.

## Transcription

- **Engine.** whisper.cpp through `whisper-rs` 0.16 with Metal. It is a
  macOS-only dependency, like recording, so Windows, Linux, and Android
  builds need no cmake or libclang; stages 6 and 7 add it there. It runs in a
  child process, re-executing the app binary with a hidden argument and an
  environment marker as `pdf_worker.rs` does, so a C++ crash or runaway memory
  use can't take down the mail client. **One long-lived worker per job** (one
  recording): the model loads once, and each track's held-back audio (below)
  survives from one segment file to the next. The worker gets an empty
  environment, `/` as its directory, no core dumps, and nice 10. The parent
  enforces a load timeout, a per-request wall-clock timeout scaled to the
  request's audio, a physical-footprint cap (polled, as macOS doesn't enforce
  `RLIMIT_AS`), and a cancel flag, and kills and reaps the worker on any
  breach, on malformed output, and on drop. **Protocol**: length-prefixed JSON
  frames both ways. A `config` frame first (model paths, language, threads),
  answered by `ready`; then one `segment` request at a time (track, WAV path,
  `startMs`, whether it continues the previous file of the track's run, and
  whether it ends the run), each answered by its `lines` (`track`, `startMs`,
  `endMs`, `text`). The parent checks every path: the exact file name the
  segment writer gives, resolved inside the recording's audio folder.
- **Speech across segment files.** Speech that reaches the end of a segment
  file is held back with the last half second of every file and transcribed
  with the next contiguous file of the same run, so a sentence is never cut at
  a minute boundary; the run's last file flushes it. What is held never
  exceeds one 30-second span.
- **Builds.** `.cargo/config.toml` (at the root and in `src-tauri`) sets
  `GGML_NATIVE=OFF`, so whisper.cpp is compiled for the architecture's
  baseline, not the build machine's CPU, and `CMAKE_OSX_DEPLOYMENT_TARGET=10.15`,
  because whisper.cpp uses `std::filesystem` and fails to compile for the
  10.13 deployment target Tauri passes to release builds.
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
- **During the recording.** Finished minute-segments are transcribed in the
  background while recording continues, so the transcript is nearly ready when
  the user presses Stop. On battery, this waits until Stop (IOKit's providing
  power source). One job runs at a time: waiting `recorded` and `interrupted`
  recordings go first, oldest first, whenever the chosen model is ready.
  Nothing of the work during a recording is visible until Stop; then the row
  shows `transcribing` with a percentage that counts what was already done.
  A job keeps the model it started with. A job that fails during the
  recording starts over after Stop.
- **Merging tracks.** Segments are ordered by start time. When every output is
  headphones, the microphone hears only the user, so nothing need be dropped.
  anarlog uses the same headphones test to switch its echo canceller off.
  **Deferred:** that exemption needs output-route detection, which can't be
  tested in the VM, so stage 3 always applies the echo rule when the
  recording has system audio. On headphones the rule rarely fires, because the
  user's words seldom repeat the other side's. Otherwise the microphone picks
  up the room's speakers. The merge works
  sentence by sentence. A microphone sentence is dropped when at least half of
  its words (0.5, in the policy module) also appear in system sentences that
  overlap it in time, ±1 second. In the spike's simulated echo (−16 dB,
  60 ms), this dropped all 13 echo sentences (scores 0.70–1.00). It kept the
  user's line spoken over the remote speaker (0.25) and the user's line alone
  (0.00). Comparing whole 30-second spans instead lost the line spoken over
  the remote speaker. This is pure, testable code. Two kinds of
  echo cancellation are ruled out for v1:
  - Apple's voice-processing I/O, because it lowers all other audio by about
    8 dB, the call included, with no public way to stop it (anarlog measured
    this).
  - A neural echo canceller like anarlog's (ONNX models run through `ort`),
    which is the next step only if the spike measures too much duplication.
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

## AI recap

- A new `Feature::MeetingRecap` (`meeting_recap`), an `AiModelUse`
  `meetingRecap` defaulting to the reasoning model, and a module
  `src-tauri/src/ai/meeting_recap.rs`. It follows `ai/meeting.rs`: a system
  prompt that treats the transcript as untrusted evidence, structured output,
  `with_purpose` usage logging, request controls, and the daily spending limit.
- **Input**: the merged transcript as numbered segments
  (`{id, speaker: "you"|"others", start, text}`), plus the reviewed event title,
  time, and attendee display names. **Include My Notes**, off by default, adds
  what the user typed in this occurrence's note excerpt during the meeting, as
  anarlog's summary does. A note marked **Exclude from AI** is never offered.
  Nothing else is sent: no mail and no contact notes. The summary's length
  guidance scales with the transcript's length (anarlog).
- **Output schema**: `summary` (120 words at most), `decisions[]`, `actions[]`
  (`owner: you|attendee|unclear`), and `openQuestions[]`, each with non-empty
  `segmentIds`. Validation checks that every cited ID exists. An item that
  fails is withheld and counted ("2 suggestions withheld"), never shown as an
  empty result.
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
  `track`, `start_ms`, `end_ms`, `text`, `dropped_as_echo`).
- **Settings** live natively in a single-row `meeting_recording_settings`
  table, because the tray, retention, and transcription read them while the
  window may be hidden. The webview reads and writes them through
  `recording_settings` and `set_recording_settings`. Fields added later get
  serde defaults.
- **Device-local.** These tables are not replicated entities and not part of
  settings transfer. The only things that leave the device are what the user
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
- Audio and transcripts stay on the device. Nothing reaches an AI provider
  until the user turns on **Summarize with AI**, and then only the transcript
  text and the reviewed event fields are sent.
- Transcript text is untrusted. It is rendered as text, and instructions spoken
  in a meeting cannot authorize writes or tools.
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
                               worker), engine (whisper, macOS), plan, merge,
                               scheduler, power
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
   Transcript**. *Built 2026-11-27*; the headphones exemption is deferred (see
   Merging tracks). Smoke-tested in the VM with the Fast model on Metal: RTF
   about 0.33 on the paravirtual GPU, model load 4.6 s cold and 0.15 s warm.
   Real Apple Silicon speed and memory are still to measure on the host.
4. **Recap and review.** `meeting_recap.rs`, validation, the Summary field and
   Suggested items in `MeetingFollowThroughDialog`, **Attach Transcript**, and
   ad-hoc destinations.
5. **Offers.** "Record “…”" for the meeting under way, and the badge when
   another app starts using the microphone during a calendar meeting. On macOS
   this works as in anarlog:
   - a listener on the default input's `kAudioDevicePropertyDeviceIsRunningSomewhere`;
   - Core Audio's process list, to name the app using the microphone;
   - a short delay before the offer, so a brief microphone check doesn't
     trigger it.

   On Windows the offer comes from WASAPI session enumeration. When the call
   app releases the microphone, the tray offers **Stop Recording** and never
   stops on its own. anarlog's documentation warns that automatic stop can
   fire early.
6. **Windows.** WASAPI loopback, tray, and installer permissions.
7. **Linux.** The monitor source and the tray (AppIndicator), with
   `docs/linux-builds.md` updated.
8. **Optional: cloud transcription.** OpenAI-compatible
   `/audio/transcriptions` for providers that offer it, with per-minute prices
   in the spending limit, behind its own switch and disclosure.

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
  stop, double start, stop while transcribing, quit while recording); segment
  writer recovery from a truncated final file; gap timestamps; policy limits
  below, at, and above each one (length, disk floor, transcript bytes); model
  hash rejection; the worker protocol with malformed frames, timeout, and kill;
  merge ordering and echo drop at the similarity threshold; recap schema
  validation, withheld counts, and unknown segment IDs; prompt-injection text in
  a transcript; retention sweep; migration.
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
- **Build and supply chain.** whisper.cpp brings a C/C++ build (cmake), Metal
  shaders, and a larger binary. If vetting or CI cost is unacceptable, the
  fallback is the stage 8 cloud path, or macOS 26's on-device `SpeechAnalyzer`
  through a small Swift shim (macOS only).
- **Echo and quality** without headphones. Measured in the spike; the merge
  rule is the first defense.
- **Bluetooth headsets.** Recording from a headset's microphone forces it
  into its low-quality call mode for the whole stream. Some headsets also cut
  the microphone between words. Settings notes this when the input is a
  Bluetooth device. Both references learned this the hard way.
- **System audio permission has no public check.** anarlog asks a private TCC
  framework, which this plan avoids. If the tap delivers only silence for the
  first minute while the microphone hears speech, the app shows a notice
  pointing to the permission in System Settings.
- **Resource use** during long calls. Transcription waits until Stop on
  battery, and the worker runs at low priority.
- **macOS API surface.** Process taps are recent and lightly documented, and
  hardened-runtime entitlements must be right before notarization.
