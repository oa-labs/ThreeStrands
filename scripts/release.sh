#!/usr/bin/env bash
# Release ThreeStrands from the host Mac without running Node on it.
#
# The checks and changelog stamping (`node scripts/release.mjs stamp`) run in
# the tart-vm macOS VM against the committed master. This script then shows
# the stamped CHANGELOG.md, and on confirmation commits it, pushes master,
# creates the signed tag, and pushes the tag, all with the host's Git, signing
# key, and credentials. `pnpm release` still does the same in one step where
# Node is available.
#
# Usage: scripts/release.sh [--yes] [--dry-run]   release the version in package.json
#        scripts/release.sh docs                  list unreleased names missing from the docs
#   --yes      skip the confirmation prompt
#   --dry-run  stop after showing the stamped changelog; changes nothing
set -Eeuo pipefail

TART_VM="${TART_VM:-tart-vm}"
REMOTE="${THREESTRANDS_RELEASE_REMOTE:-primary}"
GUEST_CLONE="${THREESTRANDS_VM_CLONE:-/Users/admin/Source/threestrands}"
GUEST_WORK="$GUEST_CLONE-release"

die() { echo "error: $*" >&2; exit 1; }
git() { command git -C "$repo_root" "$@"; }

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(command git -C "$script_dir" rev-parse --show-toplevel)"

mode=release yes=0 dry_run=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    docs) mode=docs; shift ;;
    --yes) yes=1; shift ;;
    --dry-run) dry_run=1; shift ;;
    *) sed -n '/^# Usage/,/^set /p' "$0" | grep '^#' | sed 's/^# \{0,1\}//'; exit 1 ;;
  esac
done

command -v "$TART_VM" >/dev/null || die "tart-vm is not on PATH (or set TART_VM to its path)"

# Run `node scripts/release.mjs <args>` in the VM on commit $1 and print its
# stdout. The VM sees only commits, through its read-only view of this repo.
in_vm() {
  local commit="$1"
  shift
  "$TART_VM" mac start --no-graphics >/dev/null
  {
    printf 'clone=%q work=%q commit=%q\n' "$GUEST_CLONE" "$GUEST_WORK" "$commit"
    printf 'args=(%s)\n' "$(printf '%q ' "$@")"
    cat <<'REMOTE'
set -Eeuo pipefail
[[ -e "$clone/.git" ]] || { echo "error: no clone at $clone; add a 'threestrands' mount and run tart-vm mac provision" >&2; exit 1; }
git -C "$clone" fetch --quiet origin
git -C "$clone" cat-file -e "$commit^{commit}" 2>/dev/null ||
  { echo "error: the VM cannot see commit $commit; is it committed in the mounted checkout?" >&2; exit 1; }
git -C "$clone" worktree remove --force "$work" >/dev/null 2>&1 || rm -rf "$work"
git -C "$clone" worktree prune
git -C "$clone" worktree add --quiet --detach "$work" "$commit"
trap 'git -C "$clone" worktree remove --force "$work" >/dev/null 2>&1 || true' EXIT
cd "$work"
node scripts/release.mjs "${args[@]}"
REMOTE
  } | "$TART_VM" mac ssh 'bash -s'
}

head="$(git rev-parse HEAD)"

if [[ "$mode" == docs ]]; then
  [[ -z "$(git status --porcelain -- CHANGELOG.md site)" ]] ||
    echo "warning: uncommitted CHANGELOG.md or site/ changes are not checked; the VM sees only commits" >&2
  in_vm "$head" docs
  exit 0
fi

branch="$(git branch --show-current)"
if [[ "$branch" != master ]]; then
  [[ "$dry_run" == 1 ]] || die "switch to master before releasing; the release tag must point to the branch being pushed"
  echo "warning: on $branch, not master (allowed only with --dry-run)" >&2
fi
[[ -z "$(git status --porcelain)" ]] ||
  die "commit or stash your changes before releasing; the release version must be committed"

git fetch --quiet "$REMOTE"
if [[ "$branch" == master ]] && git rev-parse -q --verify "refs/remotes/$REMOTE/master" >/dev/null &&
  ! git merge-base --is-ancestor "$REMOTE/master" HEAD; then
  die "master is behind or has diverged from $REMOTE/master; pull first"
fi

# plutil reads JSON, so no Node is needed for this.
version="$(plutil -extract version raw -o - "$repo_root/package.json")"
tag="v$version"
if git rev-parse -q --verify "refs/tags/$tag" >/dev/null ||
  [[ -n "$(git ls-remote --tags "$REMOTE" "refs/tags/$tag")" ]]; then
  die "tag $tag already exists; bump the version first"
fi

echo "Checking and stamping $tag in the VM (commit ${head:0:8})"
stamped="$(mktemp "${TMPDIR:-/tmp}/threestrands-changelog.XXXXXX")"
trap 'rm -f "$stamped"' EXIT
in_vm "$head" stamp "$(date +%Y-%m-%d)" >"$stamped"
[[ -s "$stamped" ]] || die "the VM returned an empty changelog"

notes=0
if ! cmp -s "$stamped" "$repo_root/CHANGELOG.md"; then
  notes=1
  echo
  echo "Release notes from the VM (review them; they become the commit):"
  diff -u "$repo_root/CHANGELOG.md" "$stamped" | sed '1,2d' || true
  echo
else
  echo "No unreleased changelog entries to stamp."
fi

if [[ "$dry_run" == 1 ]]; then
  echo "Dry run: nothing committed, pushed, or tagged."
  exit 0
fi

if [[ "$yes" != 1 ]]; then
  if [[ "$notes" == 1 ]]; then
    prompt="Commit these notes, push master to $REMOTE, and push signed tag $tag? [y/N] "
  else
    prompt="Push master to $REMOTE and push signed tag $tag? [y/N] "
  fi
  read -r -p "$prompt" answer
  [[ "$answer" == y || "$answer" == Y ]] || { echo "Cancelled; nothing changed."; exit 1; }
fi

if [[ "$notes" == 1 ]]; then
  cp "$stamped" "$repo_root/CHANGELOG.md"
  git add CHANGELOG.md
  git commit -m "Release notes for $tag"
fi
git push "$REMOTE" master
git tag -s "$tag" -m "ThreeStrands $tag"
git push "$REMOTE" "$tag"
echo "Released $tag"
