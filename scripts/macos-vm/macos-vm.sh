#!/usr/bin/env bash
# Manage a Tart macOS VM for development. Runs on the host Mac.
#
# The VM gets read-only views of this repository and of any repositories in
# the mounts file, and works in its own clones, so
# npm/Cargo code that runs during builds can read the sources but cannot
# write to the host checkout (including .git/hooks) or reach the host's
# files and keychain. Work comes back to the host with `fetch`, which only
# copies Git objects; it never runs code from the VM.
#
# Usage: macos-vm.sh <command>   (symlink it onto PATH as `macos-vm`)
#   create    clone the Xcode image, size the VM, and provision it
#   provision rerun the toolchain setup (safe to repeat)
#   build     build a committed ThreeStrands branch in the VM and copy the
#             app back [--branch NAME] [--release] [--sign]
#   start     boot the VM in a window (pass --no-graphics for headless)
#   stop      shut the VM down
#   ssh       open a shell in the VM (extra args are run as a command)
#   fetch     fetch the VM's branches into refs/remotes/tart/*
#   pull-app  copy the VM's debug .app (or --release) to artifacts/macos-tart/
#   sign-app  sign that ThreeStrands copy on the host with Developer ID
#   mounts    list the extra read-only mounts from the mounts file
#   ip        print the VM's address
set -Eeuo pipefail

VM_NAME="${MACOS_VM_NAME:-macos-vm}"
# Xcode 26 matches the macos-26 release runner (Icon Composer support).
VM_IMAGE="${MACOS_VM_IMAGE:-ghcr.io/cirruslabs/macos-tahoe-xcode:26.5}"
VM_CPUS="${MACOS_VM_CPUS:-6}"
VM_MEMORY_MB="${MACOS_VM_MEMORY_MB:-16384}"
# Empty keeps the image's disk (140 GB, sparse); Tart can only grow it.
VM_DISK_GB="${MACOS_VM_DISK_GB:-}"
VM_USER=admin
SSH_KEY="$HOME/.ssh/macos-vm"
SHARE_NAME=threestrands
GUEST_SHARE="/Volumes/My Shared Files/$SHARE_NAME"
GUEST_SOURCE="/Users/$VM_USER/Source"
GUEST_CLONE="$GUEST_SOURCE/threestrands"
# `build` uses its own worktree of the clone, leaving the clone alone.
GUEST_BUILD="$GUEST_SOURCE/threestrands-build"
# Other host repositories to share read-only, one absolute path per line.
# Each appears as /Volumes/My Shared Files/<folder name> and is cloned to
# ~/Source/<folder name> by `provision`.
MOUNTS_FILE="${MACOS_VM_MOUNTS_FILE:-$HOME/.config/macos-vm/mounts}"

# Resolve a PATH symlink (e.g. ~/.local/bin/macos-vm) back to the repository.
script_path="$(readlink -f "${BASH_SOURCE[0]}" 2>/dev/null || echo "${BASH_SOURCE[0]}")"
script_dir="$(cd "$(dirname "$script_path")" && pwd)"
# Share the primary checkout (not a worktree) so the VM sees every branch.
repo_root="$(cd "$(git -C "$script_dir" rev-parse --path-format=absolute --git-common-dir)/.." && pwd)"

die() { echo "error: $*" >&2; exit 1; }

require_tart() {
  command -v tart >/dev/null || die "Tart is not installed: brew install cirruslabs/cli/tart"
}

vm_exists() { tart list --quiet 2>/dev/null | grep -qx "$VM_NAME"; }

vm_running() { tart list 2>/dev/null | awk -v n="$VM_NAME" '$2 == n && $NF == "running" { found = 1 } END { exit !found }'; }

vm_ip() { tart ip --wait 120 "$VM_NAME"; }

ssh_opts=(-i "$SSH_KEY" -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new
  -o "UserKnownHostsFile=$HOME/.ssh/macos-vm-known-hosts")

vm_ssh() { ssh "${ssh_opts[@]}" "$VM_USER@$(vm_ip)" "$@"; }

# Fills EXTRA_NAMES and EXTRA_PATHS from the mounts file. (macOS ships
# bash 3.2, so no associative arrays.)
EXTRA_NAMES=()
EXTRA_PATHS=()
load_extra_mounts() {
  EXTRA_NAMES=()
  EXTRA_PATHS=()
  [[ -f "$MOUNTS_FILE" ]] || return 0
  local line path name existing
  while IFS= read -r line || [[ -n "$line" ]]; do
    line="${line%%#*}"
    line="$(printf '%s' "$line" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')"
    [[ -n "$line" ]] || continue
    path="${line/#\~/$HOME}"
    [[ "$path" == /* ]] || die "$MOUNTS_FILE: not an absolute path: $line"
    [[ -d "$path" ]] || die "$MOUNTS_FILE: no such directory: $path"
    name="$(basename "$path")"
    [[ "$name" =~ ^[A-Za-z0-9._-]+$ ]] || die "$MOUNTS_FILE: unsupported folder name: $name"
    [[ "$name" != "$SHARE_NAME" && "$name" != "threestrands-build" ]] ||
      die "$MOUNTS_FILE: $name is reserved"
    for existing in ${EXTRA_NAMES[@]+"${EXTRA_NAMES[@]}"}; do
      [[ "$existing" != "$name" ]] || die "$MOUNTS_FILE: two folders named $name"
    done
    EXTRA_NAMES+=("$name")
    EXTRA_PATHS+=("$path")
  done <"$MOUNTS_FILE"
}

cmd_mounts() {
  load_extra_mounts
  echo "$SHARE_NAME -> $repo_root (read-only)"
  local i
  for ((i = 0; i < ${#EXTRA_NAMES[@]}; i++)); do
    echo "${EXTRA_NAMES[i]} -> ${EXTRA_PATHS[i]} (read-only)"
  done
  echo "Mounts file: $MOUNTS_FILE"
}

start_vm() {
  if vm_running; then return; fi
  load_extra_mounts
  local log="${TMPDIR:-/tmp}/tart-$VM_NAME.log"
  # :ro keeps every host folder read-only inside the VM. Tart fixes mounts
  # at boot, so changes to the mounts file need a stop and start.
  local dir_args=(--dir="$SHARE_NAME:$repo_root:ro") i
  for ((i = 0; i < ${#EXTRA_NAMES[@]}; i++)); do
    dir_args+=(--dir="${EXTRA_NAMES[i]}:${EXTRA_PATHS[i]}:ro")
  done
  nohup tart run "$@" "${dir_args[@]}" "$VM_NAME" >"$log" 2>&1 &
  echo "Starting $VM_NAME (log: $log)"
  vm_ip >/dev/null
}

wait_for_ssh() {
  local ip
  ip="$(vm_ip)"
  for _ in $(seq 1 60); do
    if nc -z -G 2 "$ip" 22 >/dev/null 2>&1; then return; fi
    sleep 2
  done
  die "SSH did not come up on $ip"
}

install_ssh_key() {
  [[ -f "$SSH_KEY" ]] || ssh-keygen -q -t ed25519 -N "" -C "macos-vm" -f "$SSH_KEY"
  if vm_ssh -o BatchMode=yes true 2>/dev/null; then return; fi
  # Cirrus Labs images run the Tart guest agent, so no password is needed.
  for _ in $(seq 1 30); do
    if tart exec -i "$VM_NAME" sh -c \
      'mkdir -p ~/.ssh && chmod 700 ~/.ssh && cat >> ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys' \
      <"$SSH_KEY.pub" 2>/dev/null; then
      vm_ssh -o BatchMode=yes true && return
    fi
    sleep 2
  done
  die "could not install the SSH key through the Tart guest agent"
}

cmd_create() {
  require_tart
  vm_exists && die "$VM_NAME already exists (delete it with: tart delete $VM_NAME)"
  echo "Cloning $VM_IMAGE (a large download)"
  tart clone "$VM_IMAGE" "$VM_NAME"
  tart set "$VM_NAME" --cpu "$VM_CPUS" --memory "$VM_MEMORY_MB"
  if [[ -n "$VM_DISK_GB" ]]; then tart set "$VM_NAME" --disk-size "$VM_DISK_GB"; fi
  cmd_provision
  echo
  echo "Done. Next:"
  echo "  Open a shell:        macos-vm ssh"
  echo "  Change its password: macos-vm ssh -t passwd"
}

cmd_provision() {
  require_tart
  vm_exists || die "$VM_NAME does not exist (run: macos-vm create)"
  start_vm --no-graphics
  wait_for_ssh
  install_ssh_key
  scp "${ssh_opts[@]}" "$script_dir/provision.sh" "$VM_USER@$(vm_ip):/tmp/macos-vm-provision.sh"
  load_extra_mounts
  # Names are limited to [A-Za-z0-9._-], so they need no quoting.
  vm_ssh "bash /tmp/macos-vm-provision.sh '$GUEST_SHARE' '$GUEST_CLONE' ${EXTRA_NAMES[*]+${EXTRA_NAMES[*]}}"
}

# `tart stop` alone can hit macOS's shutdown prompt and force the VM off
# after 30 seconds, losing writes that were not yet on disk. Shut macOS down
# from inside first, and force it only as a last resort.
cmd_stop() {
  require_tart
  vm_running || { echo "$VM_NAME is not running"; return 0; }
  vm_ssh -o ConnectTimeout=10 'sync && sudo -n shutdown -h now' >/dev/null 2>&1 || true
  local _
  for _ in $(seq 1 60); do
    vm_running || { echo "$VM_NAME stopped"; return 0; }
    sleep 2
  done
  echo "warning: $VM_NAME did not shut down; forcing it off" >&2
  tart stop --timeout 30 "$VM_NAME"
}

cmd_fetch() {
  require_tart
  GIT_SSH_COMMAND="ssh ${ssh_opts[*]}" git -C "$repo_root" fetch "ssh://$VM_USER@$(vm_ip)$GUEST_CLONE" \
    "+refs/heads/*:refs/remotes/tart/*"
  echo "Fetched into refs/remotes/tart/*. Review with: git log -p master..tart/<branch>"
}

cmd_pull_app() {
  require_tart
  # Debug builds need no OAuth credentials; pass --release for a release build.
  local profile=debug
  [[ "${1:-}" == "--release" ]] && profile=release
  pull_app_from "$GUEST_CLONE" "$profile"
}

pull_app_from() {
  local dest="$repo_root/artifacts/macos-tart"
  local bundle="$1/src-tauri/target/$2/bundle/macos"
  mkdir -p "$dest"
  rm -rf "$dest/ThreeStrands.app"
  vm_ssh "tar -C '$bundle' -cf - ThreeStrands.app" | tar -C "$dest" -xf -
  echo "Copied to $dest/ThreeStrands.app"
}

# Sign the pulled app on the host so the Developer ID key never enters the VM.
# codesign only hashes the bundle's files; it runs nothing from inside it.
cmd_sign_app() {
  local app="$repo_root/artifacts/macos-tart/ThreeStrands.app"
  local binary="$app/Contents/MacOS/threestrands"
  [[ -f "$binary" ]] || die "no app at $app (run: macos-vm pull-app)"
  # The bundle came from the VM; refuse links that could point elsewhere.
  [[ -z "$(find "$app" -type l -print -quit)" ]] || die "$app contains symbolic links"
  local identity="${THREESTRANDS_SIGNING_IDENTITY:-$(sed -n 's/.*"signingIdentity": *"\([^"]*\)".*/\1/p' "$repo_root/src-tauri/tauri.conf.json")}"
  [[ -n "$identity" ]] || die "no signingIdentity in src-tauri/tauri.conf.json"
  # Match Tauri: hardened runtime and a secure timestamp, binary first.
  codesign --force --options runtime --timestamp --sign "$identity" "$binary"
  codesign --force --options runtime --timestamp --sign "$identity" "$app"
  codesign --verify --strict --verbose=2 "$app"
  echo "Signed $app with $identity"
}

cmd_build() {
  require_tart
  local branch="" profile=debug sign=0
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --branch) [[ $# -ge 2 ]] || die "--branch needs a name"; branch="$2"; shift 2 ;;
      --release) profile=release; shift ;;
      --sign) sign=1; shift ;;
      *) die "unknown build option: $1" ;;
    esac
  done
  if [[ -z "$branch" ]]; then
    branch="$(git branch --show-current 2>/dev/null || true)"
    [[ -n "$branch" ]] || die "not on a branch; pass --branch NAME"
    if [[ -n "$(git status --porcelain --untracked-files=no 2>/dev/null)" ]]; then
      echo "warning: uncommitted changes are not built; the VM sees only commits" >&2
    fi
  fi
  git -C "$repo_root" check-ref-format --branch "$branch" >/dev/null || die "invalid branch: $branch"
  git -C "$repo_root" rev-parse --verify --quiet "refs/heads/$branch" >/dev/null ||
    die "no local branch $branch in $repo_root"

  local build_args=(--bundles app --config '{"bundle":{"macOS":{"signingIdentity":"-"}}}')
  local env_lines=""
  if [[ "$profile" == debug ]]; then
    build_args=(--debug "${build_args[@]}")
  else
    for name in THREESTRANDS_GOOGLE_CLIENT_ID THREESTRANDS_GOOGLE_CLIENT_SECRET; do
      [[ -n "${!name:-}" ]] || die "$name is required for --release (set it in this shell)"
      env_lines+="export $name=$(printf '%q' "${!name}")"$'\n'
    done
  fi

  start_vm --no-graphics
  wait_for_ssh
  echo "Building $branch ($profile) in $VM_NAME"
  # The script travels on stdin, so the OAuth values never appear in a
  # command line on either machine.
  {
    printf '%s' "$env_lines"
    printf 'branch=%q clone=%q build=%q\n' "$branch" "$GUEST_CLONE" "$GUEST_BUILD"
    printf 'build_args=(%s)\n' "$(printf '%q ' "${build_args[@]}")"
    cat <<'REMOTE'
set -Eeuo pipefail
git -C "$clone" fetch --quiet --prune origin
if [[ -d "$build/.git" || -f "$build/.git" ]]; then
  # Ignored build output (node_modules, target) stays for incremental builds.
  git -C "$build" checkout --quiet --force --detach "origin/$branch"
  git -C "$build" clean --quiet -fd
else
  git -C "$clone" worktree add --quiet --detach "$build" "origin/$branch"
fi
cd "$build"
echo "Commit: $(git log -1 --format='%h %s')"
pnpm install --frozen-lockfile
pnpm tauri build "${build_args[@]}"
REMOTE
  } | vm_ssh 'bash -s'

  pull_app_from "$GUEST_BUILD" "$profile"
  if [[ "$sign" == 1 ]]; then cmd_sign_app; fi
}

command="${1:-}"
[[ $# -gt 0 ]] && shift
case "$command" in
  create) cmd_create ;;
  build) cmd_build "$@" ;;
  provision) cmd_provision ;;
  start) require_tart; start_vm "$@"; wait_for_ssh; echo "Running at $(vm_ip)" ;;
  stop) cmd_stop ;;
  ssh) require_tart; vm_ssh "$@" ;;
  fetch) cmd_fetch ;;
  pull-app) cmd_pull_app "$@" ;;
  sign-app) cmd_sign_app ;;
  mounts) cmd_mounts ;;
  ip) require_tart; vm_ip ;;
  *) sed -n '/^# Usage/,/^set /p' "$0" | grep '^#' | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
