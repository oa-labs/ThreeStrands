# Meeting recording spike (throwaway)

Stage 0 of [the meeting recording plan](../../docs/meeting-recording-plan.md).
The findings are in
[docs/meeting-recording-spike-findings.md](../../docs/meeting-recording-spike-findings.md).

This crate is its own Cargo workspace with its own lockfile, so none of its
dependencies enter the app's dependency graph, `cargo vet`, or CI. Nothing
here is shipped. Delete the directory once stage 3 lands.

| Binary | What it does |
| --- | --- |
| `capture <dir> [seconds] [--no-mic] [--no-tap]` | Records the microphone (`cpal`) and system audio (a Core Audio process tap through `objc2-core-audio`) into separate 16 kHz mono WAV tracks, printing levels every second. |
| `transcribe --model <ggml> [--vad <silero>] [--timestamps] [--tsv out] [--ref text] [--no-gpu] <wav>…` | Runs Silero VAD, then whisper.cpp on each speech span, and prints timing, real-time factor, and word error rate against a reference. |
| `merge <mic.tsv> <system.tsv> [threshold]` | Labels the tracks You and Others, and drops microphone segments that are speaker echo. |
| `probe [seconds]` | Shows whether the default input is in use, and which processes are using audio input or output. |

Build (in the VM, use the shared target directory):

```sh
CARGO_TARGET_DIR=~/Source/.cargo-target/spike CARGO_INCREMENTAL=0 cargo build --release
```

`run-on-host.sh` runs the measurements the VM can't make: a real call, speaker
echo, Metal speed, and the permission prompt. See the findings doc.
