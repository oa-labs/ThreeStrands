#!/usr/bin/env bash
# Build ThreeStrands in the tart-vm macOS VM and sign it on the host.
# Runs on the host Mac. Needs the `tart-vm` tool on PATH (or TART_VM set to
# its path) and a project named `threestrands` for this repository's primary
# checkout, e.g. `threestrands=/Users/you/Source/dispatch` in
# ~/.config/tart-vm/mac-projects. `build` syncs it to the VM first.
#
# Usage: scripts/vm-build.sh <command>
#   build     build a committed branch in the VM and copy the app back
#             [--branch NAME] [--release] [--sign] [--notarize] [--dmg]
#   pull-app  copy the VM clone's debug app (or --release) to artifacts/
#   sign-app  sign the copied app on the host with the Developer ID identity
#   notarize  notarize and staple the signed copy on the host
#   dmg       package the notarized copy as a signed, notarized disk image
#             named ThreeStrands_<version>_<arch>.dmg
#
# Signing and notarization credentials stay on the host; nothing APPLE_* is
# sent to the VM. Notarizing uses the notarytool keychain profile named by
# APPLE_KEYCHAIN_PROFILE (create one with `xcrun notarytool store-credentials`).
set -Eeuo pipefail

TART_VM="${TART_VM:-tart-vm}"
GUEST_CLONE="${THREESTRANDS_VM_CLONE:-/Users/admin/Source/threestrands}"
# `build` uses its own worktree of the clone, leaving the clone alone.
GUEST_BUILD="${THREESTRANDS_VM_BUILD:-$GUEST_CLONE-build}"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# The VM sees the primary checkout, so resolve branches there.
repo_root="$(cd "$(git -C "$script_dir" rev-parse --path-format=absolute --git-common-dir)/.." && pwd)"
APP_DIR="$repo_root/artifacts/macos-vm"

die() { echo "error: $*" >&2; exit 1; }

command -v "$TART_VM" >/dev/null || die "tart-vm is not on PATH (or set TART_VM to its path)"

pull_app_from() {
  local bundle="$1/src-tauri/target/$2/bundle/macos"
  mkdir -p "$APP_DIR"
  rm -rf "$APP_DIR/ThreeStrands.app"
  "$TART_VM" mac ssh "tar -C '$bundle' -cf - ThreeStrands.app" | tar -C "$APP_DIR" -xf -
  echo "Copied to $APP_DIR/ThreeStrands.app"
}

cmd_pull_app() {
  # Debug builds need no OAuth credentials; pass --release for a release build.
  local profile=debug
  [[ "${1:-}" == "--release" ]] && profile=release
  pull_app_from "$GUEST_CLONE" "$profile"
}

# The Developer ID identity: THREESTRANDS_SIGNING_IDENTITY, or Tauri's config.
signing_identity() {
  local identity="${THREESTRANDS_SIGNING_IDENTITY:-$(sed -n 's/.*"signingIdentity": *"\([^"]*\)".*/\1/p' "$repo_root/src-tauri/tauri.conf.json")}"
  [[ -n "$identity" ]] || die "no signingIdentity in src-tauri/tauri.conf.json"
  printf '%s\n' "$identity"
}

# Sign the pulled app on the host so the Developer ID key never enters the VM.
# codesign only hashes the bundle's files; it runs nothing from inside it.
cmd_sign_app() {
  local app="$APP_DIR/ThreeStrands.app"
  local binary="$app/Contents/MacOS/threestrands"
  [[ -f "$binary" ]] || die "no app at $app (run: scripts/vm-build.sh build)"
  # The bundle came from the VM; refuse links that could point elsewhere.
  [[ -z "$(find "$app" -type l -print -quit)" ]] || die "$app contains symbolic links"
  local identity
  identity="$(signing_identity)"
  # Match Tauri: hardened runtime and a secure timestamp, binary first.
  codesign --force --options runtime --timestamp --sign "$identity" "$binary"
  codesign --force --options runtime --timestamp --sign "$identity" "$app"
  codesign --verify --strict --verbose=2 "$app"
  echo "Signed $app with $identity"
}

# Submit a file to Apple's notary service from the host and wait for the
# verdict. Credentials come from a notarytool keychain profile, so no password
# appears in a command line or environment variable.
submit_for_notarization() {
  local file="$1" keychain_profile="${APPLE_KEYCHAIN_PROFILE:-}"
  [[ -n "$keychain_profile" ]] || die "set APPLE_KEYCHAIN_PROFILE to a notarytool keychain profile (xcrun notarytool store-credentials <name> --apple-id <id> --team-id <team>)"
  local result status id
  result="$(mktemp "${TMPDIR:-/tmp}/threestrands-notarize.XXXXXX")"
  echo "Submitting $file to Apple's notary service (profile $keychain_profile); this can take a few minutes"
  xcrun notarytool submit "$file" --keychain-profile "$keychain_profile" \
    --wait --output-format plist >"$result" || true
  status="$(plutil -extract status raw -o - "$result" 2>/dev/null || true)"
  id="$(plutil -extract id raw -o - "$result" 2>/dev/null || true)"
  rm -f "$result"
  if [[ "$status" != Accepted ]]; then
    echo "Notarization ${status:-failed}${id:+ (submission $id)}" >&2
    [[ -z "$id" ]] || xcrun notarytool log "$id" --keychain-profile "$keychain_profile" >&2 || true
    die "notarization of $file did not succeed"
  fi
  echo "Notarized $file (submission $id)"
}

# Fail unless the file is validly signed with a Developer ID identity.
require_developer_id() {
  codesign --verify --strict "$1" 2>/dev/null || die "$1 is not validly signed (run: scripts/vm-build.sh sign-app)"
  local details
  details="$(codesign -dvv "$1" 2>&1)"
  [[ "$details" == *$'\nAuthority=Developer ID Application'* ]] ||
    die "$1 is not signed with a Developer ID identity (run: scripts/vm-build.sh sign-app)"
}

# Notarize the signed app and staple the ticket.
cmd_notarize() {
  local app="$APP_DIR/ThreeStrands.app"
  [[ -d "$app" ]] || die "no app at $app (run: scripts/vm-build.sh build --sign)"
  require_developer_id "$app"
  local tmp
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/threestrands-notarize.XXXXXX")"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" EXIT
  ditto -c -k --sequesterRsrc --keepParent "$app" "$tmp/ThreeStrands.zip"
  submit_for_notarization "$tmp/ThreeStrands.zip"
  xcrun stapler staple "$app"
  spctl --assess --type execute -vv "$app"
  echo "Notarized and stapled $app"
}

# Package the notarized app as a drag-to-install disk image named like
# Tauri's (ThreeStrands_<version>_<arch>.dmg), then sign, notarize, and
# staple the image itself so Gatekeeper accepts it offline.
cmd_dmg() {
  local app="$APP_DIR/ThreeStrands.app"
  local binary="$app/Contents/MacOS/threestrands"
  [[ -f "$binary" ]] || die "no app at $app (run: scripts/vm-build.sh build --notarize)"
  require_developer_id "$app"
  xcrun stapler validate -q "$app" 2>/dev/null || die "$app has no stapled notarization ticket (run: scripts/vm-build.sh notarize)"
  # Read the version from the built app, which may come from another branch
  # than the host checkout.
  local version arch
  version="$(plutil -extract CFBundleShortVersionString raw -o - "$app/Contents/Info.plist")"
  [[ "$version" =~ ^[0-9A-Za-z.+-]+$ ]] || die "unexpected app version: $version"
  arch="$(lipo -archs "$binary")"
  case "$arch" in
    arm64) arch=aarch64 ;;
    x86_64) arch=x64 ;;
    *) arch=universal ;;
  esac
  local identity
  identity="$(signing_identity)"

  local dmg="$APP_DIR/ThreeStrands_${version}_${arch}.dmg" stage
  stage="$(mktemp -d "${TMPDIR:-/tmp}/threestrands-dmg.XXXXXX")"
  ditto "$app" "$stage/ThreeStrands.app"
  ln -s /Applications "$stage/Applications"
  rm -f "$dmg"
  hdiutil create -quiet -volname ThreeStrands -srcfolder "$stage" -fs HFS+ -format UDZO "$dmg" ||
    { rm -rf "$stage"; die "hdiutil could not create $dmg"; }
  rm -rf "$stage"
  codesign --force --timestamp --sign "$identity" "$dmg"
  submit_for_notarization "$dmg"
  xcrun stapler staple "$dmg"
  spctl --assess --type open --context context:primary-signature -vv "$dmg"
  echo "Created $dmg"
}

cmd_build() {
  local branch="" profile=debug sign=0 dmg=0
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --branch) [[ $# -ge 2 ]] || die "--branch needs a name"; branch="$2"; shift 2 ;;
      --release) profile=release; shift ;;
      --sign) (( sign >= 1 )) || sign=1; shift ;;
      --notarize) sign=2; shift ;;
      # A disk image for other machines needs a notarized app inside.
      --dmg) dmg=1; sign=2; shift ;;
      *) die "unknown build option: $1" ;;
    esac
  done
  # Debug and release images would share a name, so ship only release ones.
  [[ "$dmg" == 0 || "$profile" == release ]] || die "--dmg needs --release"
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
  local env_lines="" name
  if [[ "$profile" == debug ]]; then
    build_args=(--debug "${build_args[@]}")
  else
    for name in THREESTRANDS_GOOGLE_CLIENT_ID THREESTRANDS_GOOGLE_CLIENT_SECRET; do
      [[ -n "${!name:-}" ]] || die "$name is required for --release (set it in this shell)"
      env_lines+="export $name=$(printf '%q' "${!name}")"$'\n'
    done
  fi

  "$TART_VM" mac start --no-graphics
  "$TART_VM" mac sync --quiet threestrands
  echo "Building $branch ($profile) in the VM"
  # The script travels on stdin, so the OAuth values never appear in a
  # command line on either machine.
  {
    printf '%s' "$env_lines"
    printf 'branch=%q clone=%q build=%q\n' "$branch" "$GUEST_CLONE" "$GUEST_BUILD"
    printf 'build_args=(%s)\n' "$(printf '%q ' "${build_args[@]}")"
    cat <<'REMOTE'
set -Eeuo pipefail
[[ -e "$clone/.git" ]] || { echo "error: no clone at $clone; add a 'threestrands' project and run tart-vm mac sync" >&2; exit 1; }
git -C "$clone" fetch --quiet --prune origin
if [[ -e "$build/.git" ]]; then
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
  } | "$TART_VM" mac ssh 'bash -s'

  pull_app_from "$GUEST_BUILD" "$profile"
  if [[ "$sign" -ge 1 ]]; then cmd_sign_app; fi
  if [[ "$sign" == 2 ]]; then cmd_notarize; fi
  if [[ "$dmg" == 1 ]]; then cmd_dmg; fi
}

command="${1:-}"
[[ $# -gt 0 ]] && shift
case "$command" in
  build) cmd_build "$@" ;;
  pull-app) cmd_pull_app "$@" ;;
  sign-app) cmd_sign_app ;;
  notarize) cmd_notarize ;;
  dmg) cmd_dmg ;;
  *) sed -n '/^# Usage/,/^set /p' "$0" | grep '^#' | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
