#!/usr/bin/env bash
set -Eeuo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_root"

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "error: Linux packages must be built inside the Dispatch devcontainer" >&2
  exit 1
fi

if [[ "$(uname -m)" != "x86_64" ]]; then
  echo "error: the initial Linux release target is x86_64; current architecture is $(uname -m)" >&2
  exit 1
fi

release_build="${DISPATCH_RELEASE_BUILD:-0}"
if [[ "$release_build" != "0" && "$release_build" != "1" ]]; then
  echo "error: DISPATCH_RELEASE_BUILD must be 0 or 1" >&2
  exit 1
fi

if [[ "$release_build" == "1" ]]; then
  if [[ -z "${DISPATCH_GOOGLE_CLIENT_ID:-}" ]]; then
    echo "error: DISPATCH_GOOGLE_CLIENT_ID is required for a release build" >&2
    exit 1
  fi
  if [[ -z "${DISPATCH_GOOGLE_CLIENT_SECRET:-}" ]]; then
    echo "error: DISPATCH_GOOGLE_CLIENT_SECRET is required for a release build" >&2
    exit 1
  fi
fi

echo "Building Dispatch Linux x86-64 packages"
echo "Node:  $(node --version)"
echo "pnpm:  $(pnpm --version)"
echo "Rust:  $(rustc --version)"
echo "Cargo: $(cargo --version)"
echo "Tauri: $(pnpm exec tauri --version)"

pnpm install --frozen-lockfile
pnpm test
cargo test --locked --manifest-path src-tauri/Cargo.toml

build_marker="$(mktemp)"
cleanup_paths=("$build_marker")
cleanup() {
  local path
  for path in "${cleanup_paths[@]}"; do
    rm -rf -- "$path"
  done
}
trap cleanup EXIT

pnpm tauri build --bundles deb,rpm,appimage

bundle_root="$project_root/src-tauri/target/release/bundle"
artifact_dir="$project_root/artifacts/linux-amd64"
mkdir -p "$artifact_dir"
find "$artifact_dir" -mindepth 1 -maxdepth 1 -type f -delete

mapfile -d '' packages < <(
  find "$bundle_root" -type f -newer "$build_marker" \
    \( -name '*.deb' -o -name '*.rpm' -o -name '*.AppImage' \) -print0
)

for extension in deb rpm AppImage; do
  found=0
  for package in "${packages[@]}"; do
    if [[ "$package" == *."$extension" ]]; then
      found=1
      break
    fi
  done
  if [[ "$found" != "1" ]]; then
    echo "error: Tauri did not produce a .$extension package" >&2
    exit 1
  fi
done

for package in "${packages[@]}"; do
  cp -p "$package" "$artifact_dir/"
done

deb_package="$(find "$artifact_dir" -maxdepth 1 -type f -name '*.deb' -print -quit)"
rpm_package="$(find "$artifact_dir" -maxdepth 1 -type f -name '*.rpm' -print -quit)"
appimage_package="$(find "$artifact_dir" -maxdepth 1 -type f -name '*.AppImage' -print -quit)"
native_binary="$project_root/src-tauri/target/release/dispatch"

if [[ "$(dpkg-deb -f "$deb_package" Architecture)" != "amd64" ]]; then
  echo "error: Debian package is not amd64" >&2
  exit 1
fi

if [[ "$(rpm -qp --queryformat '%{ARCH}' "$rpm_package")" != "x86_64" ]]; then
  echo "error: RPM package is not x86_64" >&2
  exit 1
fi

if ! file "$appimage_package" | grep -Eq 'x86-64|x86_64'; then
  echo "error: AppImage is not x86-64" >&2
  exit 1
fi

if ! file "$native_binary" | grep -Eq 'x86-64|x86_64'; then
  echo "error: native executable is not x86-64" >&2
  exit 1
fi

dpkg-deb -I "$deb_package" > "$artifact_dir/debian-package-info.txt"
rpm -qpR "$rpm_package" > "$artifact_dir/rpm-requires.txt"
ldd "$native_binary" > "$artifact_dir/native-library-dependencies.txt"

appimage_extract_dir="$(mktemp -d)"
cleanup_paths+=("$appimage_extract_dir")
(
  cd "$appimage_extract_dir"
  "$appimage_package" --appimage-extract >/dev/null
)
test -f "$appimage_extract_dir/squashfs-root/AppRun"

(
  cd "$artifact_dir"
  sha256sum ./*.deb ./*.rpm ./*.AppImage > SHA256SUMS
)

echo "Linux artifacts:"
find "$artifact_dir" -maxdepth 1 -type f -print | sort
