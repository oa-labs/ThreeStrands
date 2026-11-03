#!/usr/bin/env bash
# Install the ThreeStrands toolchain inside the Tart VM and clone the repo
# from the host's read-only share. Run by `scripts/tart/dev-vm.sh create`.
# Versions match .devcontainer/Dockerfile and the release workflow.
set -Eeuo pipefail

share="$1"
clone="$2"

# Non-interactive SSH sessions and sudo leave /usr/local/bin off PATH.
export PATH="/usr/local/bin:$HOME/.cargo/bin:$PATH"

NODE_VERSION=22.22.0
NODE_DARWIN_ARM64_SHA256=2bd596bbfc4a275ceb8721a5954ee97daea5ebe673e96a185ebd732f6fb023ac
PNPM_VERSION=12.16.1

[[ "$(uname -m)" == "arm64" ]] || { echo "error: expected an arm64 macOS guest" >&2; exit 1; }
[[ -d "$share/.git" ]] || { echo "error: host share not mounted at $share" >&2; exit 1; }

xcodebuild -version

if [[ "$(node --version 2>/dev/null)" != "v$NODE_VERSION" ]]; then
  archive="node-v$NODE_VERSION-darwin-arm64.tar.xz"
  tmp="$(mktemp -d)"
  curl -fsSL -o "$tmp/$archive" "https://nodejs.org/dist/v$NODE_VERSION/$archive"
  echo "$NODE_DARWIN_ARM64_SHA256  $tmp/$archive" | shasum -a 256 -c -
  sudo mkdir -p /usr/local
  sudo tar -xJf "$tmp/$archive" -C /usr/local --strip-components=1
  rm -rf "$tmp"
fi
sudo env "PATH=$PATH" npm install --global "pnpm@$PNPM_VERSION"

if ! command -v rustup >/dev/null && [[ ! -x "$HOME/.cargo/bin/rustup" ]]; then
  # The toolchain itself comes from rust-toolchain.toml on first use.
  curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
fi
# ~/.zshenv also covers `dev-vm.sh ssh <command>` and `fetch`, which run
# non-interactive shells.
grep -q 'threestrands-toolchain' "$HOME/.zshenv" 2>/dev/null ||
  echo 'export PATH="/usr/local/bin:$HOME/.cargo/bin:$PATH" # threestrands-toolchain' >>"$HOME/.zshenv"

# The share is owned by the host's user ID, which Git otherwise refuses.
git config --global --add safe.directory "$share"
if [[ ! -d "$clone/.git" ]]; then
  git clone "$share" "$clone"
fi
cd "$clone"
rustup toolchain install
# Default to the pinned toolchain outside the repo too.
rustup default "$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)"
pnpm install --frozen-lockfile

echo
echo "Node $(node --version), pnpm $(pnpm --version), $(rustc --version)"
echo "Repository: $clone (origin is the host's read-only checkout)"
