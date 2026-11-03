#!/usr/bin/env bash
# Install development toolchains inside the macOS VM and clone the host's
# read-only shares. Run by `scripts/macos-vm/macos-vm.sh create|provision`;
# safe to repeat. Every download is pinned to a version and checksum.
#
#   Node + pnpm, Rust   ThreeStrands (match .devcontainer/Dockerfile)
#   Temurin JDK 21      talaria (Gradle and Kotlin come from its wrapper)
#   .NET SDK 10 + 8/9   oa-payroll-tools (runtimes for net8.0/net9.0 tests)
set -Eeuo pipefail

share="$1"
clone="$2"
shift 2
# Remaining arguments name extra read-only shares to clone beside $clone.
extra_names=("$@")

NODE_VERSION=22.22.0
NODE_DARWIN_ARM64_SHA256=2bd596bbfc4a275ceb8721a5954ee97daea5ebe673e96a185ebd732f6fb023ac
PNPM_VERSION=12.16.1

JDK_VERSION=21.0.13+9
JDK_URL=https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.13%2B9/OpenJDK21U-jdk_aarch64_mac_hotspot_21.0.13_9.tar.gz
JDK_SHA256=bf036b35b071bfb2f54f2c0af8ee8d88afa95ee6eb3dce02c0f94cfc2524d04a
JDK_HOME="$HOME/Library/Java/JavaVirtualMachines/temurin-21.jdk/Contents/Home"

DOTNET_ROOT="$HOME/.dotnet"
# "<directory that marks it installed> <url> <sha512>", from Microsoft's
# release metadata (builds.dotnet.microsoft.com/dotnet/release-metadata).
DOTNET_PACKAGES=(
  "sdk/10.0.403 https://builds.dotnet.microsoft.com/dotnet/Sdk/10.0.403/dotnet-sdk-10.0.403-osx-arm64.tar.gz 5893c66b48ea3674c7da18501059d606e96f3672fabe4d5e41762e692c7c83df3ee0b2b9dc5a0786191da2dc413d7335158101c14c935ab217f31ac2529770f3"
  "shared/Microsoft.NETCore.App/9.0.22 https://builds.dotnet.microsoft.com/dotnet/Runtime/9.0.22/dotnet-runtime-9.0.22-osx-arm64.tar.gz 93953db94e59aa6c89a514abb02f7f35cbcd4752c613db7df4f90f5e5ff7f83b1bab071d3eb998bb4b0c56caa80ea65482c7d62e81f06c6680e164ff88416bf8"
  "shared/Microsoft.NETCore.App/8.0.33 https://builds.dotnet.microsoft.com/dotnet/Runtime/8.0.33/dotnet-runtime-8.0.33-osx-arm64.tar.gz 0be37b64ea1f49651bbaa3dd08e89476919ceb98bd6b39d28883439c16c1d9cbcccf5cf01f67c913095e9f23235dc621125a551a6e736be4ebddbf060eb82758"
)

# Non-interactive SSH sessions and sudo leave these off PATH.
export PATH="/usr/local/bin:$HOME/.cargo/bin:$DOTNET_ROOT:$JDK_HOME/bin:$PATH"

[[ "$(uname -m)" == "arm64" ]] || { echo "error: expected an arm64 macOS guest" >&2; exit 1; }
[[ -d "$share/.git" ]] || { echo "error: host share not mounted at $share" >&2; exit 1; }

xcodebuild -version

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# download <url> <file> <sha-bits> <checksum>
download() {
  curl -fsSL -o "$2" "$1"
  echo "$4  $2" | shasum -a "$3" -c -
}

if [[ "$(node --version 2>/dev/null)" != "v$NODE_VERSION" ]]; then
  archive="$tmp/node.tar.xz"
  download "https://nodejs.org/dist/v$NODE_VERSION/node-v$NODE_VERSION-darwin-arm64.tar.xz" "$archive" 256 "$NODE_DARWIN_ARM64_SHA256"
  sudo mkdir -p /usr/local
  sudo tar -xJf "$archive" -C /usr/local --strip-components=1
fi
sudo env "PATH=$PATH" npm install --global "pnpm@$PNPM_VERSION"

if ! command -v rustup >/dev/null && [[ ! -x "$HOME/.cargo/bin/rustup" ]]; then
  # The toolchain itself comes from rust-toolchain.toml on first use.
  curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
fi

if ! "$JDK_HOME/bin/java" -version 2>&1 | grep -qF "$JDK_VERSION"; then
  archive="$tmp/jdk.tar.gz"
  download "$JDK_URL" "$archive" 256 "$JDK_SHA256"
  jdk_bundle="$(dirname "$(dirname "$JDK_HOME")")"
  rm -rf "$jdk_bundle"
  mkdir -p "$jdk_bundle"
  tar -xzf "$archive" -C "$jdk_bundle" --strip-components=1
fi
"$JDK_HOME/bin/java" -version 2>&1 | head -1

mkdir -p "$DOTNET_ROOT"
for entry in "${DOTNET_PACKAGES[@]}"; do
  read -r marker url sha512 <<<"$entry"
  [[ -d "$DOTNET_ROOT/$marker" ]] && continue
  archive="$tmp/dotnet.tar.gz"
  download "$url" "$archive" 512 "$sha512"
  tar -xzf "$archive" -C "$DOTNET_ROOT"
done
DOTNET_CLI_TELEMETRY_OPTOUT=1 "$DOTNET_ROOT/dotnet" --list-sdks
DOTNET_CLI_TELEMETRY_OPTOUT=1 "$DOTNET_ROOT/dotnet" --list-runtimes

# ~/.zshenv also covers `macos-vm.sh ssh <command>` and `fetch`, which run
# non-interactive shells. Rewrite the managed block on every run.
touch "$HOME/.zshenv"
sed -i '' -e '/# >>> macos-vm toolchains/,/# <<< macos-vm toolchains/d' -e '/# threestrands-toolchain$/d' "$HOME/.zshenv"
cat >>"$HOME/.zshenv" <<'ZSHENV'
# >>> macos-vm toolchains (managed by scripts/macos-vm/provision.sh)
export JAVA_HOME="$HOME/Library/Java/JavaVirtualMachines/temurin-21.jdk/Contents/Home"
export DOTNET_ROOT="$HOME/.dotnet"
export DOTNET_CLI_TELEMETRY_OPTOUT=1
[ -d "$HOME/android-sdk" ] && export ANDROID_HOME="$HOME/android-sdk"
export PATH="/usr/local/bin:$HOME/.cargo/bin:$DOTNET_ROOT:$DOTNET_ROOT/tools:$JAVA_HOME/bin:$PATH"
# <<< macos-vm toolchains
ZSHENV

# The shares are owned by the host's user ID, which Git otherwise refuses.
add_safe_directory() {
  git config --global --get-all safe.directory | grep -qxF "$1" ||
    git config --global --add safe.directory "$1"
}

add_safe_directory "$share"
mkdir -p "$(dirname "$clone")"
if [[ ! -d "$clone/.git" ]]; then
  git clone "$share" "$clone"
fi
cd "$clone"
rustup toolchain install
# Default to the pinned toolchain outside the repo too.
rustup default "$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)"
pnpm install --frozen-lockfile

for name in ${extra_names[@]+"${extra_names[@]}"}; do
  src="$(dirname "$share")/$name"
  dest="$(dirname "$clone")/$name"
  if [[ ! -d "$src/.git" ]]; then
    echo "warning: $src is not mounted or not a Git repository; restart the VM after editing the mounts file" >&2
    continue
  fi
  add_safe_directory "$src"
  if [[ -d "$dest/.git" ]]; then
    echo "Repository: $dest (already cloned)"
  else
    git clone --quiet "$src" "$dest"
    echo "Repository: $dest (cloned; origin is the host's read-only checkout)"
  fi
done

echo
echo "Node $(node --version), pnpm $(pnpm --version), $(rustc --version)"
echo "Repository: $clone (origin is the host's read-only checkout)"
