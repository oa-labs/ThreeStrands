#!/bin/sh
# Cargo `runner` for macOS dev builds (see ../.cargo/config.toml).
#
# `tauri dev` launches the app via `cargo run`, which produces a freshly
# unsigned (or ad-hoc signed) binary on every rebuild. macOS Keychain ties its
# "Always Allow" access-control decision to the running binary's code
# signature, so a signature that changes every build means a Keychain prompt
# every build too. Signing with the same stable local identity on every run
# keeps the signature constant, so the OS only has to ask once.
#
# The identity below is a free, self-signed "Code Signing" certificate meant
# for local development only (see PLAN.MD or ask Claude to recreate it) — it
# is not a Developer ID certificate and only satisfies local Keychain ACLs,
# not Gatekeeper/notarization for distributed builds. It is named "Dispatch
# Dev Signing" to match the certificate in this machine's keychain.
set -e

binary="$1"
shift

codesign --force --sign "Dispatch Dev Signing" --options runtime "$binary"

exec "$binary" "$@"
