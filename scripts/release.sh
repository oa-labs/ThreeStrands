#!/usr/bin/env bash
# Release ThreeStrands from the host Mac without running Node on it.
#
# The checks, version bump, and changelog stamping
# (`node scripts/release.mjs bump`) run in the tart-vm macOS VM against the
# committed master, after syncing it there. This script then shows the
# resulting changes, and on confirmation applies and commits them, pushes
# master, creates the signed tag, and pushes the tag, all with the host's Git,
# signing key, and credentials. `pnpm release` still does the same in one step
# where Node is available.
#
# Usage: scripts/release.sh [--yes] [--dry-run] [--major | VERSION]
#                                         release the next version: minor when
#                                         a pending entry is New, otherwise patch
#        scripts/release.sh docs          list unreleased names missing from the docs
#   --yes      skip the confirmation prompt
#   --dry-run  stop after showing the release changes; changes nothing
#   --major    release the next major version
#   VERSION    release exactly this version; the current version re-cuts it
set -Eeuo pipefail

TART_VM="${TART_VM:-tart-vm}"
REMOTE="${THREESTRANDS_RELEASE_REMOTE:-origin}"
GUEST_CLONE="${THREESTRANDS_VM_CLONE:-/Users/admin/Source/threestrands}"
GUEST_WORK="$GUEST_CLONE-release"

die() { echo "error: $*" >&2; exit 1; }
git() { command git -C "$repo_root" "$@"; }

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(command git -C "$script_dir" rev-parse --show-toplevel)"

mode=release yes=0 dry_run=0 choice=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    docs) mode=docs; shift ;;
    --yes) yes=1; shift ;;
    --dry-run) dry_run=1; shift ;;
    --major) choice+=(--major); shift ;;
    [0-9]*.*) choice+=("$1"); shift ;;
    *) sed -n '/^# Usage/,/^set /p' "$0" | grep '^#' | sed 's/^# \{0,1\}//'; exit 1 ;;
  esac
done
[[ ${#choice[@]} -le 1 ]] || die "pass at most one of --major or VERSION"

command -v "$TART_VM" >/dev/null || die "tart-vm is not on PATH (or set TART_VM to its path)"

# Run `node scripts/release.mjs <args>` in the VM on commit $1 and print its
# stdout. The VM sees only commits, which `tart-vm mac sync` sends it.
in_vm() {
  local commit="$1"
  shift
  "$TART_VM" mac start --no-graphics >/dev/null
  "$TART_VM" mac sync --quiet threestrands >&2
  {
    printf 'clone=%q work=%q commit=%q\n' "$GUEST_CLONE" "$GUEST_WORK" "$commit"
    printf 'args=(%s)\n' "$(printf '%q ' "$@")"
    cat <<'REMOTE'
set -Eeuo pipefail
[[ -e "$clone/.git" ]] || { echo "error: no clone at $clone; add a 'threestrands' project and run tart-vm mac sync" >&2; exit 1; }
git -C "$clone" fetch --quiet origin
git -C "$clone" cat-file -e "$commit^{commit}" 2>/dev/null ||
  { echo "error: the VM cannot see commit $commit after syncing" >&2; exit 1; }
git -C "$clone" worktree remove --force "$work" >/dev/null 2>&1 || rm -rf "$work"
git -C "$clone" worktree prune
git -C "$clone" worktree add --quiet --detach "$work" "$commit"
trap 'git -C "$clone" worktree remove --force "$work" >/dev/null 2>&1 || true' EXIT
cd "$work"
node scripts/release.mjs "${args[@]}"
# bump changes the worktree; send those changes back after its tag line.
if [[ "${args[0]}" == bump ]]; then
  git add -A
  git diff --cached --binary
fi
REMOTE
  } | "$TART_VM" mac ssh 'bash -s'
}

head="$(git rev-parse HEAD)"

if [[ "$mode" == docs ]]; then
  [[ -z "$(git status --porcelain -- CHANGELOG.md changes site)" ]] ||
    echo "warning: uncommitted CHANGELOG.md, changes/, or site/ changes are not checked; the VM sees only commits" >&2
  in_vm "$head" docs
  exit 0
fi

branch="$(git branch --show-current)"
if [[ "$branch" != master ]]; then
  [[ "$dry_run" == 1 ]] || die "switch to master before releasing; the release tag must point to the branch being pushed"
  echo "warning: on $branch, not master (allowed only with --dry-run)" >&2
fi
[[ -z "$(git status --porcelain)" ]] ||
  die "commit or stash your changes before releasing; the release starts from what master has"

git fetch --quiet "$REMOTE"
if [[ "$branch" == master ]] && git rev-parse -q --verify "refs/remotes/$REMOTE/master" >/dev/null &&
  ! git merge-base --is-ancestor "$REMOTE/master" HEAD; then
  die "master is behind or has diverged from $REMOTE/master; pull first"
fi

echo "Checking and preparing the release in the VM (commit ${head:0:8})"
output="$(mktemp "${TMPDIR:-/tmp}/threestrands-release.XXXXXX")"
patch="$output.patch"
trap 'rm -f "$output" "$patch"' EXIT
in_vm "$head" bump "$(date +%Y-%m-%d)" ${choice[@]+"${choice[@]}"} >"$output"
tag="$(head -n 1 "$output")"
[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-beta\.[0-9]+)?$ ]] || die "the VM did not return a release tag (got: ${tag:-nothing})"
tail -n +2 "$output" >"$patch"

if git rev-parse -q --verify "refs/tags/$tag" >/dev/null ||
  [[ -n "$(git ls-remote --tags "$REMOTE" "refs/tags/$tag")" ]]; then
  die "tag $tag already exists; to re-cut it, delete it first (see DEVELOPERS.md)"
fi

changes=0
if [[ -s "$patch" ]]; then
  changes=1
  git apply --check --index "$patch" || die "the VM's release changes don't apply to this checkout"
  echo
  echo "Release changes for $tag from the VM (review them; they become the commit):"
  cat "$patch"
  echo
else
  echo "Re-cutting $tag: no version or changelog changes."
fi

if [[ "$dry_run" == 1 ]]; then
  echo "Dry run: nothing committed, pushed, or tagged."
  exit 0
fi

if [[ "$yes" != 1 ]]; then
  if [[ "$changes" == 1 ]]; then
    prompt="Commit these changes, push master to $REMOTE, and push signed tag $tag? [y/N] "
  else
    prompt="Push master to $REMOTE and push signed tag $tag? [y/N] "
  fi
  read -r -p "$prompt" answer
  [[ "$answer" == y || "$answer" == Y ]] || { echo "Cancelled; nothing changed."; exit 1; }
fi

if [[ "$changes" == 1 ]]; then
  git apply --index "$patch"
  git commit -m "Release $tag"
fi
git push "$REMOTE" master
git tag -s "$tag" -m "ThreeStrands $tag"
git push "$REMOTE" "$tag"
echo "Released $tag"
