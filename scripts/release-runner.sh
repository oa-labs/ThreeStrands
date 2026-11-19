#!/usr/bin/env bash
# Run ThreeStrands release builds on this Mac, one throwaway tart VM per job.
#
# Runs on the host Mac, not in the development VM. For each job it clones the
# base VM, starts the clone, registers a single-use (just-in-time) GitHub
# Actions runner inside it, waits for that runner to take and finish one job,
# then stops and deletes the clone. Nothing the job saw, including the signing
# secrets it received, outlives the job.
#
# Usage: scripts/release-runner.sh [--jobs N]
#   --jobs N  stop after N jobs; by default it serves until interrupted
#
# Needs tart, gh signed in with admin access to the repository, and ssh key
# access to the base VM's user. Settings, from the environment:
#   RUNNER_BASE_VM   base VM to clone        (threestrands-release-base)
#   RUNNER_REPO      repository              (oa-labs/ThreeStrands)
#   RUNNER_LABEL     label the release workflow targets (threestrands-release)
#   RUNNER_SSH_USER  user in the base VM     (admin)
#   RUNNER_DIR       runner folder in that user's home (actions-runner)
# See "Release builds on your own Mac" in DEVELOPERS.md for the base VM.
set -Eeuo pipefail

TART="${TART:-tart}"
GH="${GH:-gh}"
SSH="${SSH:-ssh}"
BASE_VM="${RUNNER_BASE_VM:-threestrands-release-base}"
REPO="${RUNNER_REPO:-oa-labs/ThreeStrands}"
LABEL="${RUNNER_LABEL:-threestrands-release}"
SSH_USER="${RUNNER_SSH_USER:-admin}"
RUNNER_DIR="${RUNNER_DIR:-actions-runner}"

die() { echo "error: $*" >&2; exit 1; }

jobs=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --jobs)
      [[ "${2:-}" =~ ^[1-9][0-9]*$ ]] || die "--jobs needs a positive number"
      jobs="$2"; shift 2 ;;
    *) sed -n '/^# Usage/,/^#   --jobs/p' "$0" | sed 's/^# \{0,1\}//'; exit 1 ;;
  esac
done

for tool in "$TART" "$GH" "$SSH"; do
  command -v "$tool" >/dev/null || die "$tool is not on PATH"
done
"$TART" get "$BASE_VM" >/dev/null 2>&1 ||
  die "no base VM named $BASE_VM; see \"Release builds on your own Mac\" in DEVELOPERS.md"

# The clone and runner of the job in progress, for cleanup.
vm="" vm_pid="" runner_id=""

cleanup() {
  if [[ -n "$runner_id" ]]; then
    # A runner that never took a job stays registered; remove it. GitHub has
    # already removed one that finished its job, so a failure here is fine.
    "$GH" api -X DELETE "repos/$REPO/actions/runners/$runner_id" >/dev/null 2>&1 || true
    runner_id=""
  fi
  if [[ -n "$vm" ]]; then
    "$TART" stop "$vm" >/dev/null 2>&1 || true
    [[ -z "$vm_pid" ]] || wait "$vm_pid" 2>/dev/null || true
    "$TART" delete "$vm" >/dev/null 2>&1 || echo "warning: could not delete VM $vm; run: $TART delete $vm" >&2
    vm="" vm_pid=""
  fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

vm_ssh() {
  "$SSH" -o BatchMode=yes -o ConnectTimeout=5 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    -o LogLevel=ERROR "$SSH_USER@$1" "${@:2}"
}

serve_one() {
  local ip config jit
  vm="threestrands-release-$(date +%Y%m%d-%H%M%S)-$$-$((served + 1))"
  echo "Starting $vm from $BASE_VM"
  "$TART" clone "$BASE_VM" "$vm"
  "$TART" run --no-graphics "$vm" >/dev/null 2>&1 &
  vm_pid=$!
  ip="$("$TART" ip --wait 120 "$vm")" || die "$vm did not get an IP address"
  local ready=0
  for _ in $(seq 60); do
    if vm_ssh "$ip" true 2>/dev/null; then ready=1; break; fi
    sleep 2
  done
  [[ "$ready" == 1 ]] || die "cannot reach $SSH_USER@$ip over ssh; is your key in the base VM?"

  # A just-in-time runner takes exactly one job, then GitHub removes it.
  config="$("$GH" api -X POST "repos/$REPO/actions/runners/generate-jitconfig" \
    -f name="$vm" -F runner_group_id=1 -f work_folder=_work \
    -f 'labels[]=self-hosted' -f 'labels[]=macOS' -f 'labels[]=ARM64' -f "labels[]=$LABEL" \
    --jq '"\(.runner.id) \(.encoded_jit_config)"')"
  runner_id="${config%% *}"
  jit="${config#* }"
  [[ "$runner_id" =~ ^[0-9]+$ && -n "$jit" && "$jit" != "$config" ]] || die "GitHub did not return a runner configuration"

  echo "Waiting for a release job on $vm (label $LABEL)"
  # The configuration is a credential: send it over stdin, not the command line.
  printf '%s\n' "$jit" | vm_ssh "$ip" "read -r jit && cd ~/$RUNNER_DIR && ./run.sh --jitconfig \"\$jit\""
  echo "Finished the job on $vm"
  cleanup
}

served=0
while [[ "$jobs" == 0 || "$served" -lt "$jobs" ]]; do
  serve_one
  served=$((served + 1))
done
