# Meeting recording spike findings

**Date:** 2026-11-26. **Plan:** [Meeting recording and transcription](meeting-recording-plan.md),
stage 0. **Code:** [`spikes/meeting-recording/`](../spikes/meeting-recording/README.md),
a throwaway crate with its own workspace and lockfile.

**Where it ran:** the developer's tart VM: macOS 26.6.2 on arm64, 6 vCPUs,
16 GB of memory, and Apple's Virtual Sound Device. The VM's GPU is Apple's
paravirtual device (Metal family Apple5, no simdgroup matrix support), so its
Metal timings don't represent real hardware. Its microphone is silent, and
there is no acoustic path from its speakers to its microphone.
`run-on-host.sh` covers what needs a real Mac. See [Still open](#still-open).

The test speech is a 54-second, two-topic meeting script read by macOS `say`.
Accuracy is word error rate (WER) against that script. Clean synthetic speech
is much easier than a real call, so read the WER figures as relative, not
absolute.

## Answers

### Capture

- **`objc2-core-audio` 0.3.2 is enough; `cidre` isn't needed.** Its
  `AudioHardware` feature exposes:
  - `CATapDescription`;
  - `AudioHardwareCreateProcessTap` and `AudioHardwareDestroyProcessTap`;
  - the aggregate-device keys, including the tap list;
  - `kAudioHardwarePropertyTranslatePIDToProcessObject`;
  - the process-object properties used for meeting detection.

  It is the same objc2 0.3 family the app already uses.
- **The tap recorded system audio in the VM.** The setup was a mono global tap
  that excludes this process, inside a private aggregate device. The device
  sets the default output's UID as its main sub-device, lists only the tap,
  and starts the tap automatically. Speech played with `afplay` arrived at
  −15.5 dBFS at 48 kHz mono, and the tap's own process was excluded. No
  permission prompt appeared. Whether the host shows one, and when, is still
  open.
- **The tap's floor in the SDK is macOS 14.2.**
  `AudioHardwareCreateProcessTap` is marked `API_AVAILABLE(macos(14.2))` in
  Xcode 26.5's `AudioHardwareTapping.h`. Running on 14.2 and 14.4 is untested.
- **The tap delivers nothing until some app first plays audio.** In one test
  the first callback came 5 seconds after the tap started. After that, it
  delivered continuously, silence included, through a 35-second quiet gap.
  **Consequence:** the two tracks can't be assumed to start together. Each
  track records the host time (`AudioTimeStamp.mHostTime`) of its first
  callback, and the merge aligns the tracks by those times. Counting frames
  from the start of recording is not enough.
- **Microphone:** `cpal` 0.18 opened the default input at 48 kHz, stereo, as
  f32. Callbacks never came more than 35–40 ms apart. Resampling to 16 kHz
  with one persistent `rubato` FFT resampler per track worked without
  artifacts.
- **The spike's IOProc allocates a Vec per callback.** Production must use a
  preallocated ring buffer.

### Meeting detection

- `kAudioDevicePropertyDeviceIsRunningSomewhere` on the default input
  changes from 0 to 1 while another process records.
- `kAudioHardwarePropertyProcessObjectList`, with
  `kAudioProcessPropertyIsRunningInput` and `kAudioProcessPropertyPID`,
  identifies the process that is recording.
- The bundle ID was empty for command-line tools. A real call app's would
  be filled in, and `NSRunningApplication` (already a dependency) can name it
  from the PID.

### Transcription

Models were downloaded from Hugging Face. Each file's SHA-256 matched the hash
Hugging Face publishes for it, which gives these values to pin in the policy
module:

| Model | Bytes | SHA-256 |
| --- | --- | --- |
| `ggml-small.en-q5_1.bin` (proposed **Fast**) | 190,098,681 | `bfdff4894dcb76bbf647d56263ea2a96645423f1669176f4844a1bf8e478ad30` |
| `ggml-large-v3-turbo-q5_0.bin` (proposed **Accurate**) | 574,041,195 | `394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2` |
| `ggml-base.en.bin` (measured only) | 147,964,211 | `a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002` |
| `ggml-silero-v5.1.2.bin` (VAD) | 885,098 | `29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf` |

Silero v6.2.0 is also published (`2aa269b7…6987`), and anarlog uses it. It
wasn't measured.

The table below covers the 54-second script with VAD and beam search of 5.
The real-time factor (RTF) is transcription time divided by audio length.

| Model | VM CPU RTF | VM Metal RTF | Peak memory (CPU) | WER |
| --- | --- | --- | --- | --- |
| base.en | 0.023 | 0.150 | 527 MB | 3.9% |
| small.en q5_1 | 0.04–0.06 | 0.32 | 796 MB | 1.9% |
| large-v3-turbo q5_0 | 0.09–0.15 | 0.28 | 1,317 MB | 1.9–2.6% |

- **Speed is not a constraint.** On the VM's CPU alone, a one-hour meeting
  takes about 3–4 minutes with Fast and about 9 with Accurate. The
  paravirtual GPU is slower than the CPU here. Real Apple Silicon with Metal
  needs measuring on the host.
- **whisper-rs ignores whisper.cpp's built-in VAD.** whisper.cpp applies
  `params.vad` only in `whisper_full()` and `whisper_full_parallel()`.
  whisper-rs 0.16's `WhisperState::full` calls `whisper_full_with_state`, so
  `FullParams::enable_vad` does nothing. Nothing reports this. (Also,
  `enable_vad` panics if the model path isn't set first.)
  - **Fix:** run `WhisperVadContext::segments_from_samples` separately, then
    run whisper on each speech span. Timestamps are offset by the span's
    start.
  - **Effect:** Silero took 0.12 s for 94 s of audio. On a file with
    20 seconds of noise at each end, it sent 54.5 of 93.8 seconds to whisper,
    cutting the time spent by about 40%.
  - **Hallucination:** without VAD, the padding produced a "you" line. With
    VAD, nothing.
- **Timestamp tokens: keep them on, and keep spans at 30 seconds or
  shorter.** This changes the plan, which followed meetily in turning them
  off.

  | Input | Timestamp tokens | WER |
  | --- | --- | --- |
  | 94 s without VAD | off | **44%**: whole sentences silently dropped |
  | 94 s without VAD | on | 3.2% |
  | VAD spans of 30 s or less | off | 1.9% |
  | VAD spans of 30 s or less | on | 1.9% |

  With the tokens on, whisper also splits each span into sentences, and the
  echo merge below needs those sentence-level segments. Turning them off is
  safe only when every input is 30 seconds or shorter, and it costs the
  sentence segments.
- Flash attention stayed off, per anarlog's macOS crash report. It wasn't
  tested on.

### Echo, and the You/Others merge

The VM has no acoustic path, so echo was simulated. The microphone track was
the remote speech mixed in at −16 dB, 60 ms late, plus noise and two lines of
"your" speech in another voice. One of your lines was spoken over the remote
speaker; the other was alone. Whisper transcribed the −16 dB echo almost word
for word.

| Segments | What happened |
| --- | --- |
| 30-second VAD spans (timestamp tokens off) | **Your line spoken over the remote speaker was lost.** It landed in the same span as 30 s of echo, which scored 0.85 and was dropped. |
| Sentences (timestamp tokens on) | **All 13 echo segments dropped** (scores 0.70–1.00). **Both of your lines kept**: the one spoken over the remote speaker scored 0.25, the one alone 0.00. Thresholds 0.5 and 0.6 gave the same result. |

The score is the share of a microphone segment's words that also appear in
system segments overlapping it in time, ±1 second. Recommended threshold:
**0.5**. The lowest echo score (0.70) came from a garbled echo line, so 0.5
leaves margin below it and above the overlap line (0.25).

Real rooms add reverb and level changes, and conferencing apps already cancel
some echo. `run-on-host.sh` checks this on speakers and on headphones.

### Build and supply chain

- whisper.cpp (`whisper-rs-sys` 0.15) built from clean in 15 seconds on the
  VM, with Metal. It needs cmake and libclang (bindgen) on every build
  machine, including the Windows and Linux CI runners. The binary grows by
  about 2 MB, with the Metal library embedded.
- The candidates were added to `src-tauri` temporarily (`cpal` 0.18.2,
  `rubato` 5.1.0, `hound` 3.5.1, `whisper-rs` 0.16.0 with Metal,
  `objc2-core-audio` 0.3.2, and Tauri's `tray-icon` feature), then reverted.
  `pnpm supply-chain` reported:
  - `cargo deny`: **advisories and sources pass.**
  - Crate release age: **passes.**
  - `cargo vet` (src-tauri): **35 unvetted crates.** `tray-icon` 0.25.1 is
    already locked and vetted, so the tray adds nothing.

  | Group | Crates |
  | --- | --- |
  | whisper | whisper-rs, whisper-rs-sys, plus build-only: bindgen, clang-sys, cexpr, minimal-lexical, prettyplease, cmake, fs_extra, either, itertools |
  | cpal | cpal, coreaudio-rs, objc2-audio-toolbox, objc2-avf-audio, mach2, dasp_sample, num-derive, audio-codec-algorithms, visibility, plus Linux: alsa, alsa-sys |
  | rubato (FFT resampler) | rubato, realfft, rustfft, primal-check, strength_reduce, transpose, windowfunctions, audioadapter, audioadapter-buffers, audioadapter-sample |
  | Core Audio and WAV | objc2-core-audio, objc2-core-audio-types, hound |

  Ways to shrink this, for the developer to weigh:
  - `rubato` without its default `fft_resampler` feature, using its sinc or
    polynomial resampler, drops the FFT crates.
  - On macOS, capturing the microphone through Core Audio directly, as the tap
    already does, drops cpal's macOS crates.
  - Writing WAV by hand, which is about 40 lines, drops `hound`.

  Per AGENTS.md, nothing was certified or exempted.

## Still open

Needs the developer's Mac:

1. **A real call** through `run-on-host.sh`, once on speakers and once on
   headphones: echo levels, merge quality, and Metal RTF for Fast and
   Accurate.
2. **The system-audio permission prompt:** when it appears, which app it
   names when run from Terminal versus the bundled app, and what the tap
   delivers after the user denies it.
3. **The tap floor:** the developer set it at macOS 14.4 on 2026-11-27. Run
   `capture` in a tart macOS 14.4 VM to confirm the tap records there. Below
   14.4 the app records the microphone only.

Decided without measuring:

- **Parakeet is deferred.** Whisper already runs far faster than real time on
  CPU, and comparing accuracy needs real meeting audio, not synthetic speech.
  Adding it means a second ML runtime (`ort` and the ONNX Runtime binary) to
  vet. Revisit if real-call WER disappoints.
- **Language:** the proposed **Fast** model (`small.en`) is English-only.
  **Accurate** (turbo) is multilingual. If **Fast** must handle other
  languages, use `ggml-small-q5_1.bin`; it wasn't measured.
