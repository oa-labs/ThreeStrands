# Linux builds

Dispatch produces Linux x86-64 Debian, RPM, and AppImage packages from an
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
DISPATCH_LINUX_BUNDLES=deb,rpm pnpm build:linux
```

The native x86-64 GitHub Actions runner remains the source of the AppImage and
always builds all three formats. A configured release build also requires all
three formats, so `DISPATCH_LINUX_BUNDLES=deb,rpm` is intentionally rejected
when `DISPATCH_RELEASE_BUILD=1`.

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
DISPATCH_RELEASE_BUILD=1 \
DISPATCH_GOOGLE_CLIENT_ID="1234.apps.googleusercontent.com" \
DISPATCH_GOOGLE_CLIENT_SECRET="value-from-desktop-client-json" \
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
requests. Version tags matching `v*` run in release mode and require these
GitHub Actions secrets:

- `DISPATCH_GOOGLE_CLIENT_ID`
- `DISPATCH_GOOGLE_CLIENT_SECRET`

The workflow can also be started manually. Select the `release` input only when
the repository secrets are configured. Release-mode builds fail before
compilation rather than silently producing a Gmail-disabled application.

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
