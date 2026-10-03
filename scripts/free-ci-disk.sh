#!/usr/bin/env bash
set -Eeuo pipefail

# GitHub-hosted Ubuntu runners ship ~50 GB of SDKs the devcontainer build never
# uses. The image, its OCI export, the Cargo target, and AppImage staging need
# that space. This deletes system directories, so it refuses to run elsewhere.
if [[ "${GITHUB_ACTIONS:-}" != "true" || "${RUNNER_ENVIRONMENT:-}" != "github-hosted" ]]; then
  echo "error: free-ci-disk.sh only runs on GitHub-hosted Actions runners" >&2
  exit 1
fi

echo "Disk before cleanup:"
df -h /

sudo rm -rf \
  /usr/local/lib/android \
  /usr/share/dotnet \
  /opt/ghc \
  /usr/local/.ghcup \
  /opt/hostedtoolcache \
  /usr/local/share/boost \
  /usr/share/swift \
  /usr/local/share/powershell \
  /usr/local/julia*
sudo docker image prune --all --force > /dev/null

echo "Disk after cleanup:"
df -h /
