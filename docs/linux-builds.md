# Linux builds

ThreeStrands produces Linux x86-64 Debian, RPM, and AppImage packages from an
Ubuntu 22.04 devcontainer. The same devcontainer definition is used locally
and by GitHub Actions so both builds share the Node, pnpm, Rust, native library,
and packaging toolchain.

The build toolchain pins Node 22.22.0, pnpm 12.4.1, and Rust 1.98.1.

Ubuntu 22.04 is intentionally the baseline. Building on an older supported
distribution prevents a newer host glibc from unnecessarily restricting where
the resulting AppImage can run.

## Prerequisites

- Docker Desktop, Rancher Desktop, or another Dev Container-compatible Docker
  engine
- Visual Studio Code with the Dev Containers extension, the `devcontainer`
  CLI, or another implementation of the Dev Container specification
- At least 8 GB of memory available to the container

Open the repository in its devcontainer. Its post-create command installs the
locked JavaScript dependencies. Named volumes keep Linux `node_modules` and
Rust target files separate from any macOS build outputs. The container
deliberately targets `linux/amd64`; on an Apple Silicon host, Docker therefore
uses x86-64 emulation. The build output is correct for x86-64 Linux, but
compilation is slower than it would be on a native x86-64 machine.

On Apple Silicon, the container runtime must provide working x86-64 emulation.
For Rancher Desktop, select the VZ virtual machine and enable **Rosetta
support** in Preferences > Virtual Machine > Emulation before opening the
devcontainer. QEMU user emulation can crash the Rust compiler even though
ordinary x86-64 commands appear to work.

Rosetta does not execute the statically linked x86-64 helper that Tauri uses
to finish an AppImage. On Apple Silicon, use the devcontainer to produce the
Debian and RPM packages locally:

```sh
THREESTRANDS_LINUX_BUNDLES=deb,rpm pnpm build:linux
```

The native x86-64 GitHub Actions runner remains the source of the AppImage and
always builds all three formats. A configured release build also requires all
three formats, so `THREESTRANDS_LINUX_BUNDLES=deb,rpm` is intentionally rejected
when `THREESTRANDS_RELEASE_BUILD=1`.

## Local build

Inside the devcontainer, run:

```sh
pnpm build:linux
```

This produces all three package formats on a native x86-64 host. Use the
Apple Silicon command above for local Debian/RPM validation.

This unconfigured build does not contain Google OAuth credentials. It is useful
for compile, package, and installation testing, but its Gmail connection is
disabled.

To produce a configured release build, provide the Google Desktop OAuth client
values only to the build process:

```sh
THREESTRANDS_RELEASE_BUILD=1 \
THREESTRANDS_GOOGLE_CLIENT_ID="1234.apps.googleusercontent.com" \
THREESTRANDS_GOOGLE_CLIENT_SECRET="value-from-desktop-client-json" \
pnpm build:linux
```

Do not put real values in the Dockerfile, `devcontainer.json`, a committed
environment file, or a shell script. Google calls the second value a client
secret, but installed desktop applications are public clients and cannot keep
it confidential.

The script runs the JavaScript and Rust test suites before packaging. Successful
artifacts and their SHA-256 checksums are copied to:

```text
artifacts/linux-amd64/
```

It also records Debian metadata, RPM requirements, and native dynamic-library
dependencies beside the packages.

## Continuous integration

`.github/workflows/linux-build.yml` runs an unconfigured build for pull
requests and supports manual builds. `.github/workflows/release.yml` builds
the Linux packages in release mode for version tags matching `v*` and attaches
them to a draft GitHub Release after all platforms succeed. Release-mode builds
require these GitHub Actions secrets:

- `THREESTRANDS_GOOGLE_CLIENT_ID`
- `THREESTRANDS_GOOGLE_CLIENT_SECRET`

For a manual Linux build, select the `release` input only when the repository
secrets are configured. Release-mode builds fail before compilation rather than
silently producing a Gmail-disabled application.

Both workflows select a Buildx builder with the `docker-container` driver before
running `devcontainers/ci`. Specifying `platform: linux/amd64` makes the action
export an OCI archive, which the runner's default `docker` driver cannot export
without the containerd image store. The explicit builder avoids depending on
that runner setting. See [Docker's OCI exporter documentation](https://docs.docker.com/build/exporters/oci-docker/).

Both Linux CI jobs limit Cargo to two concurrent compilation jobs, disable
incremental compilation, and use `line-tables-only` debug information for dev
and test builds. This retains filename/line-number backtraces while reducing
the test build's disk and memory requirements. Test assertions, overflow checks,
and release optimization settings retain their defaults. These overrides are
explicitly forwarded into the container and do not affect local builds.
See [Cargo's profile documentation](https://doc.rust-lang.org/cargo/reference/profiles.html).
The build script reports disk space, memory, and Rust target directory size
before building and after command failures, preserving the failing exit status.

Run the release workflow regression tests with:

```sh
node --test scripts/release.test.mjs
```

## Runtime expectations

The application uses WebKitGTK for its webview. Installed Debian and RPM
packages declare their native runtime dependencies, while AppImage bundles most
of its userspace dependencies.

OAuth tokens are stored through the freedesktop Secret Service API. A normal
GNOME or KDE desktop generally provides this through GNOME Keyring or KWallet.
Minimal desktop environments must provide and unlock a compatible Secret
Service on the user's D-Bus session.

Before publishing a release, install the generated package in a clean Linux
desktop VM and verify:

- application startup under Wayland and X11;
- Google sign-in and browser return;
- token persistence after an application restart;
- attachment open and save dialogs;
- external-link opening; and
- application icon and desktop launcher integration.

Container packaging does not replace these desktop integration checks because
the build container does not run a graphical session, browser, or keyring.

## ARM64

ARM64 Linux packages are intentionally outside the initial build target.
AppImages should be built on a native ARM64 runner rather than cross-compiled
from this x86-64 container. Add an ARM64 CI job and repeat the clean-desktop
runtime checks before advertising ARM64 support.

## Troubleshooting

- `collect2: fatal error: ld terminated with signal 7 [Bus error]` means the
  linker crashed. Check the resource report at the end of the log for disk or
  memory pressure. The signal alone does not establish whether this is resource
  exhaustion or a linker defect; keep the full linker backtrace if resources
  are available. Ensure the tag contains the current CI resource settings.
- `OCI exporter is not supported for the docker driver` means the devcontainer
  build used the default Docker builder. Ensure the Buildx setup step runs
  before `devcontainers/ci` and selects the `docker-container` driver. A release
  tag must point to a commit containing this setup; rerunning an older tag
  reuses its original workflow.
- An immediate architecture error means the container was started without its
  required `linux/amd64` platform setting. Rebuild and reopen the devcontainer.
- A Rust compiler `SIGSEGV` on Apple Silicon generally means the container
  runtime is using QEMU rather than Rosetta for x86-64 instructions. Enable the
  runtime's Rosetta support and rebuild the container.
- An AppImage `exec format error` under Rosetta is the static-helper limitation
  described above. Build `deb,rpm` locally and let the native x86-64 CI job
  produce the AppImage.
- AppImage extraction errors usually indicate a packaging failure; the build
  script uses extraction mode and does not require mounting the AppImage.
- Keyring errors at runtime indicate that no usable Secret Service is available
  or that the user's login keyring is locked.
- Build outputs under `src-tauri/target` and `artifacts` are generated and can
  be removed before a clean rebuild.
