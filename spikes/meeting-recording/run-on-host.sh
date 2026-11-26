#!/bin/sh
# Measurements the tart VM can't make: a real call through real speakers,
# Metal transcription speed, and the system-audio permission prompt.
#
#   ./run-on-host.sh <label> [seconds]
#
# Run it from Terminal during a real call, once on speakers
# (label "speakers") and once on headphones (label "headphones"). Talk at
# least once while the other side is talking. Results go to ./host-runs/<label>/.
set -eu
label=${1:?usage: run-on-host.sh <label> [seconds]}
seconds=${2:-300}
here=$(cd "$(dirname "$0")" && pwd)
models="$here/host-runs/models"
out="$here/host-runs/$label"
target=${CARGO_TARGET_DIR:-$here/target}
mkdir -p "$models" "$out"

fetch() { # name url sha256
  if [ ! -f "$models/$1" ]; then curl -fL -o "$models/$1.part" "$2" && mv "$models/$1.part" "$models/$1"; fi
  echo "$3  $models/$1" | shasum -a 256 -c - >/dev/null || { echo "hash mismatch: $1" >&2; exit 1; }
}
hf=https://huggingface.co
fetch ggml-small.en-q5_1.bin "$hf/ggerganov/whisper.cpp/resolve/main/ggml-small.en-q5_1.bin" bfdff4894dcb76bbf647d56263ea2a96645423f1669176f4844a1bf8e478ad30
fetch ggml-large-v3-turbo-q5_0.bin "$hf/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin" 394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2
fetch ggml-silero-v5.1.2.bin "$hf/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin" 29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf

(cd "$here" && CARGO_TARGET_DIR="$target" cargo build --release -q)
bin="$target/release"

sw_vers -productVersion > "$out/macos.txt"
"$bin/probe" 1 > "$out/probe.txt"
echo "Recording $seconds s. The first tap may show a permission prompt."
"$bin/capture" "$out/audio" "$seconds" | tee "$out/capture.txt"

for model in ggml-small.en-q5_1.bin ggml-large-v3-turbo-q5_0.bin; do
  for track in mic system; do
    ls "$out/audio/$track"-*.wav >/dev/null 2>&1 || continue
    /usr/bin/time -l "$bin/transcribe" --model "$models/$model" --vad "$models/ggml-silero-v5.1.2.bin" \
      --timestamps --tsv "$out/$track-$model.tsv" "$out/audio/$track"-*.wav \
      > "$out/$track-$model.txt" 2> "$out/$track-$model.time"
    tail -2 "$out/$track-$model.txt"
    grep "maximum resident" "$out/$track-$model.time"
  done
  [ -f "$out/mic-$model.tsv" ] && [ -f "$out/system-$model.tsv" ] &&
    "$bin/merge" "$out/mic-$model.tsv" "$out/system-$model.tsv" 0.5 > "$out/merged-$model.txt"
done
echo "Done. Review $out/merged-*.txt: echo lines marked drop, your own lines kept as You."
echo "Delete $out/audio when finished; it holds the call's audio."
