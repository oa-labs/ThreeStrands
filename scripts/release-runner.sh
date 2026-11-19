#!/usr/bin/env bash
# Run ThreeStrands release builds on this Mac, one throwaway tart VM per job.
#
# Runs on the host Mac, not in the development VM. For each job it clones the
# base VM, starts the clone, registers a single-use (just-in-time) GitHub
# Actions runner inside it, waits for that runner to take and finish one job,
# then stops and deletes the clone. Nothing the job saw, including the signing
# secrets it received, outlives the job.
#
# Usage: scripts/release-runner.sh [serve] [--jobs N]
#        scripts/release-runner.sh setup [--image IMAGE] [--replace]
#        scripts/release-runner.sh require-approval
#   serve             serve release jobs (the default)
#     --jobs N        stop after N jobs; by default it serves until interrupted
#   setup             require approval for outside contributors' workflows,
#                     then build the base VM from IMAGE
#     --image IMAGE   tart image with Xcode 26 or later
#                     (ghcr.io/cirruslabs/macos-tahoe-xcode:latest)
#     --replace       rebuild the base VM when it already exists
#   require-approval  only require approval for outside contributors' workflows
#
# Needs tart, gh signed in with admin access to the repository, and an SSH key
# (~/.ssh/id_ed25519.pub, id_ecdsa.pub, or id_rsa.pub). Settings, from the
# environment:
#   RUNNER_BASE_VM        base VM to clone   (threestrands-release-base)
#   RUNNER_REPO           repository         (oa-labs/ThreeStrands)
#   RUNNER_LABEL          label the release workflow targets (threestrands-release)
#   RUNNER_SSH_USER       user in the base VM (admin)
#   RUNNER_SSH_KEY        public key to install (the first of the keys above)
#   RUNNER_BASE_PASSWORD  the image's password for that user, used once (admin)
#   RUNNER_DIR            runner folder in that user's home (actions-runner)
# See "Release builds on your own Mac" in DEVELOPERS.md.
set -Eeuo pipefail

TART="${TART:-tart}"
GH="${GH:-gh}"
SSH="${SSH:-ssh}"
BASE_VM="${RUNNER_BASE_VM:-threestrands-release-base}"
REPO="${RUNNER_REPO:-oa-labs/ThreeStrands}"
LABEL="${RUNNER_LABEL:-threestrands-release}"
SSH_USER="${RUNNER_SSH_USER:-admin}"
RUNNER_DIR="${RUNNER_DIR:-actions-runner}"
IMAGE="${RUNNER_IMAGE:-ghcr.io/cirruslabs/macos-tahoe-xcode:latest}"
# Release builds need Xcode 26 for the Icon Composer app icon.
MIN_XCODE=26
APPROVAL_POLICY=all_external_contributors
SSHD_CONFIG=/etc/ssh/sshd_config.d/100-threestrands-release.conf

die() { echo "error: $*" >&2; exit 1; }
usage() { sed -n '/^# Usage/,/^#   require-approval/p' "$0" | sed 's/^# \{0,1\}//'; exit 1; }

command="serve"
case "${1:-}" in serve|setup|require-approval) command="$1"; shift ;; esac
jobs=0 replace=0
while [[ $# -gt 0 ]]; do
  case "$command:$1" in
    serve:--jobs)
      [[ "${2:-}" =~ ^[1-9][0-9]*$ ]] || die "--jobs needs a positive number"
      jobs="$2"; shift 2 ;;
    setup:--image)
      [[ -n "${2:-}" ]] || die "--image needs an image name"
      IMAGE="$2"; shift 2 ;;
    setup:--replace) replace=1; shift ;;
    *) usage ;;
  esac
done

for tool in "$TART" "$GH" "$SSH"; do
  command -v "$tool" >/dev/null || die "$tool is not on PATH"
done

# The VM and runner being worked on, for cleanup.
vm="" vm_pid="" runner_id="" askpass=""

stop_vm() {
  "$TART" stop "$vm" >/dev/null 2>&1 || true
  [[ -z "$vm_pid" ]] || wait "$vm_pid" 2>/dev/null || true
  vm_pid=""
}

cleanup() {
  if [[ -n "$runner_id" ]]; then
    # A runner that never took a job stays registered; remove it. GitHub has
    # already removed one that finished its job, so a failure here is fine.
    "$GH" api -X DELETE "repos/$REPO/actions/runners/$runner_id" >/dev/null 2>&1 || true
    runner_id=""
  fi
  if [[ -n "$vm" ]]; then
    stop_vm
    "$TART" delete "$vm" >/dev/null 2>&1 || echo "warning: could not delete VM $vm; run: $TART delete $vm" >&2
    vm=""
  fi
  [[ -z "$askpass" ]] || rm -f "$askpass"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

ssh_options=(-o ConnectTimeout=5 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR)
identity=()
[[ -z "${RUNNER_SSH_KEY:-}" ]] || identity=(-i "${RUNNER_SSH_KEY%.pub}")

vm_ssh() {
  "$SSH" -o BatchMode=yes "${ssh_options[@]}" ${identity[@]+"${identity[@]}"} "$SSH_USER@$1" "${@:2}"
}

# Logs in with the image's password, which only setup uses: it installs the
# key, then turns password logins off.
password_ssh() {
  SSH_ASKPASS="$askpass" SSH_ASKPASS_REQUIRE=force DISPLAY="${DISPLAY:-:0}" \
    RUNNER_ASKPASS_PASSWORD="${RUNNER_BASE_PASSWORD:-admin}" \
    "$SSH" -o PubkeyAuthentication=no -o PreferredAuthentications=password,keyboard-interactive \
    -o NumberOfPasswordPrompts=1 "${ssh_options[@]}" "$SSH_USER@$1" "${@:2}"
}

# Starts $vm in this shell, so cleanup can wait for it.
start_vm() {
  "$TART" run --no-graphics "$vm" >/dev/null 2>&1 &
  vm_pid=$!
}

# Prints $vm's IP address once ssh answers with `login`.
vm_address() {
  local login="$1" ip
  ip="$("$TART" ip --wait 120 "$vm")" || die "$vm did not get an IP address"
  for _ in $(seq 60); do
    if "$login" "$ip" true </dev/null 2>/dev/null; then echo "$ip"; return; fi
    sleep 2
  done
  die "cannot reach $SSH_USER@$ip over ssh"
}

require_approval() {
  echo "Requiring approval for workflows from all outside contributors in $REPO"
  "$GH" api -X PUT "repos/$REPO/actions/permissions/fork-pr-contributor-approval" \
    -f approval_policy="$APPROVAL_POLICY" >/dev/null ||
    die "could not change $REPO's fork workflow approval; gh needs admin access to the repository"
  check_approval
}

# Refuses to go on unless outside contributors' workflows wait for approval:
# the repository is public, and a fork's pull request could otherwise run its
# own workflow on this Mac.
check_approval() {
  local policy
  policy="$("$GH" api "repos/$REPO/actions/permissions/fork-pr-contributor-approval" --jq .approval_policy)" ||
    die "could not read $REPO's fork workflow approval; gh needs admin access to the repository"
  [[ "$policy" == "$APPROVAL_POLICY" ]] ||
    die "$REPO runs some outside contributors' workflows without approval ($policy); run: $0 require-approval"
}

public_key() {
  local key
  for key in ${RUNNER_SSH_KEY:+"$RUNNER_SSH_KEY"} ~/.ssh/id_ed25519.pub ~/.ssh/id_ecdsa.pub ~/.ssh/id_rsa.pub; do
    if [[ -f "$key" ]]; then echo "$key"; return; fi
  done
  die "no SSH public key found; create one with ssh-keygen -t ed25519, or set RUNNER_SSH_KEY"
}

setup() {
  local key ip version release sha url
  if "$TART" get "$BASE_VM" >/dev/null 2>&1 && [[ "$replace" != 1 ]]; then
    die "$BASE_VM already exists; pass --replace to rebuild it"
  fi
  key="$(public_key)"
  require_approval

  # The latest Actions runner for Apple Silicon, checked against GitHub's
  # published SHA-256 once it is in the VM.
  release="$("$GH" api repos/actions/runner/releases/latest --jq '
    (.tag_name | ltrimstr("v")) as $v
    | (.assets[] | select(.name == "actions-runner-osx-arm64-\($v).tar.gz")) as $a
    | (($a.digest // "") | ltrimstr("sha256:")) as $d
    | (if $d != "" then $d else (.body | capture("<!-- BEGIN SHA osx-arm64 -->(?<s>[0-9a-f]{64})<!-- END SHA osx-arm64 -->").s) end) as $sha
    | "\($v) \($sha) \($a.browser_download_url)"')" || die "could not look up the latest Actions runner"
  read -r version sha url <<<"$release"
  [[ "$version" =~ ^[0-9]+(\.[0-9]+)+$ && "$sha" =~ ^[0-9a-f]{64}$ &&
    "$url" == "https://github.com/actions/runner/releases/download/v$version/actions-runner-osx-arm64-$version.tar.gz" ]] ||
    die "unexpected Actions runner release: $release"

  # Build under a temporary name, so a failure leaves any existing base alone.
  vm="$BASE_VM-setup-$$"
  echo "Creating $vm from $IMAGE (the first pull can take a while)"
  "$TART" clone "$IMAGE" "$vm"
  askpass="$(mktemp "${TMPDIR:-/tmp}/release-runner-askpass.XXXXXX")"
  printf '#!/bin/sh\nprintf "%%s\\n" "$RUNNER_ASKPASS_PASSWORD"\n' >"$askpass"
  chmod 700 "$askpass"
  start_vm
  ip="$(vm_address password_ssh)"

  echo "Installing $key for $SSH_USER"
  password_ssh "$ip" 'umask 077 && mkdir -p ~/.ssh && cat >> ~/.ssh/authorized_keys' <"$key"
  vm_ssh "$ip" true </dev/null || die "the key was installed, but logging in with it failed"

  local xcode
  xcode="$(vm_ssh "$ip" 'xcodebuild -version' </dev/null | head -n 1)" || die "$IMAGE has no working Xcode"
  [[ "$xcode" =~ ^Xcode\ ([0-9]+) && "${BASH_REMATCH[1]}" -ge "$MIN_XCODE" ]] ||
    die "$IMAGE has ${xcode:-no Xcode}; release builds need Xcode $MIN_XCODE or later (pick another --image)"
  echo "Found $xcode"

  echo "Installing Actions runner $version in ~/$RUNNER_DIR"
  vm_ssh "$ip" "set -e; cd ~; rm -rf '$RUNNER_DIR' runner.tar.gz
curl -fsSL -o runner.tar.gz '$url'
echo '$sha  runner.tar.gz' | shasum -a 256 -c - >/dev/null
mkdir '$RUNNER_DIR' && tar -xzf runner.tar.gz -C '$RUNNER_DIR' && rm runner.tar.gz
test -x '$RUNNER_DIR/run.sh'" </dev/null || die "installing the Actions runner failed"

  # Keys only from now on. sudo may need the image's password, sent on stdin
  # ahead of the file's contents; with passwordless sudo, send only the file.
  echo "Turning off SSH password logins"
  local settings=$'PasswordAuthentication no\nKbdInteractiveAuthentication no\n'
  if vm_ssh "$ip" 'sudo -n true' </dev/null 2>/dev/null; then
    printf '%s' "$settings" | vm_ssh "$ip" "sudo -n tee $SSHD_CONFIG >/dev/null"
  else
    printf '%s\n%s' "${RUNNER_BASE_PASSWORD:-admin}" "$settings" | vm_ssh "$ip" "sudo -S -p '' tee $SSHD_CONFIG >/dev/null"
  fi || die "could not turn off SSH password logins"
  if password_ssh "$ip" true </dev/null 2>/dev/null; then
    echo "warning: $BASE_VM still accepts the password over SSH; check $SSHD_CONFIG" >&2
  fi

  stop_vm
  if "$TART" get "$BASE_VM" >/dev/null 2>&1; then "$TART" delete "$BASE_VM"; fi
  "$TART" rename "$vm" "$BASE_VM"
  vm=""
  echo "Created $BASE_VM. Serve release jobs with: $0"
}

serve_one() {
  local ip config jit
  vm="threestrands-release-$(date +%Y%m%d-%H%M%S)-$$-$((served + 1))"
  echo "Starting $vm from $BASE_VM"
  "$TART" clone "$BASE_VM" "$vm"
  start_vm
  ip="$(vm_address vm_ssh)"

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

serve() {
  "$TART" get "$BASE_VM" >/dev/null 2>&1 || die "no base VM named $BASE_VM; create it with: $0 setup"
  check_approval
  served=0
  while [[ "$jobs" == 0 || "$served" -lt "$jobs" ]]; do
    serve_one
    served=$((served + 1))
  done
}

case "$command" in
  serve) serve ;;
  setup) setup ;;
  require-approval) require_approval ;;
esac
