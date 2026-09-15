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

bundle_selection="${DISPATCH_LINUX_BUNDLES:-deb,rpm,appimage}"
case "$bundle_selection" in
  deb,rpm | deb,rpm,appimage) ;;
  *)
    echo "error: DISPATCH_LINUX_BUNDLES must be deb,rpm or deb,rpm,appimage" >&2
    exit 1
    ;;
esac

if [[ "$release_build" == "1" && "$bundle_selection" != "deb,rpm,appimage" ]]; then
  echo "error: release builds must produce deb,rpm,appimage" >&2
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
echo "Bundles: $bundle_selection"
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

pnpm tauri build --bundles "$bundle_selection"

bundle_root="$project_root/src-tauri/target/release/bundle"
artifact_dir="$project_root/artifacts/linux-amd64"
mkdir -p "$artifact_dir"
find "$artifact_dir" -mindepth 1 -maxdepth 1 -type f -delete

mapfile -d '' packages < <(
  find "$bundle_root" -type f -newer "$build_marker" \
    \( -name '*.deb' -o -name '*.rpm' -o -name '*.AppImage' \) -print0
)

expected_extensions=(deb rpm)
if [[ "$bundle_selection" == "deb,rpm,appimage" ]]; then
  expected_extensions+=(AppImage)
fi

for extension in "${expected_extensions[@]}"; do
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
  install -m 0644 "$package" "$artifact_dir/"
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

if ! file "$native_binary" | grep -Eq 'x86-64|x86_64'; then
  echo "error: native executable is not x86-64" >&2
  exit 1
fi

dpkg-deb -I "$deb_package" > "$artifact_dir/debian-package-info.txt"
rpm -qpR "$rpm_package" > "$artifact_dir/rpm-requires.txt"
ldd "$native_binary" > "$artifact_dir/native-library-dependencies.txt"

if [[ "$bundle_selection" == "deb,rpm,appimage" ]]; then
  if ! file "$appimage_package" | grep -Eq 'x86-64|x86_64'; then
    echo "error: AppImage is not x86-64" >&2
    exit 1
  fi

  appimage_extract_dir="$(mktemp -d)"
  cleanup_paths+=("$appimage_extract_dir")
  (
    cd "$appimage_extract_dir"
    "$appimage_package" --appimage-extract >/dev/null
  )
  test -f "$appimage_extract_dir/squashfs-root/AppRun"
fi

(
  cd "$artifact_dir"
  checksum_files=(./*.deb ./*.rpm)
  if [[ "$bundle_selection" == "deb,rpm,appimage" ]]; then
    checksum_files+=(./*.AppImage)
  fi
  sha256sum "${checksum_files[@]}" > SHA256SUMS
)

echo "Linux artifacts:"
find "$artifact_dir" -maxdepth 1 -type f -print | sort
