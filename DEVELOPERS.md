# Developing ThreeStrands

This guide covers building, testing, and releasing ThreeStrands from source.
For what the app does and how to install it, see the [README](README.md).
Agent and contributor rules (testing, email rendering, settings transfer, and
versioning) are in [`AGENTS.md`](AGENTS.md).

The detailed product definition, architecture, and delivery roadmap are in
[`PLAN.MD`](PLAN.MD).

## Implementation status

The browser development build runs against deterministic fixtures. The native
Tauri build uses local SQLite through the same typed client interface. See
[`docs/features.md`](docs/features.md) for current capabilities, known
limitations, and the live-account release checks. Design notes are in
[`docs/`](docs/); completed plans are in [`docs/archive/`](docs/archive/).

## Getting started

Prerequisites:

- Node.js 22 or newer
- Rust stable installed through `rustup`
- the [Tauri 2 platform prerequisites](https://v2.tauri.app/start/prerequisites/)

```sh
pnpm install
pnpm dev       # browser preview with fixture mail
pnpm lint
pnpm test
pnpm test:e2e  # first run: pnpm exec playwright install chromium
pnpm build
pnpm tauri dev # native app with local SQLite
```

Linux release packages are built in the repository's reproducible Ubuntu
devcontainer. It produces x86-64 Debian, RPM, and AppImage artifacts whether
the host is Linux or Apple Silicon. See the [Linux build guide](docs/linux-builds.md).

## Dependency safety

npm packages and Rust crates run code on the machine that builds them: npm
packages whenever a tool like Vite, Vitest, or ESLint loads them, and crates
through build scripts and procedural macros on every build, including the
background checks rust-analyzer runs in your editor.

### Development container

`.devcontainer/dev` is a native-architecture Ubuntu container (arm64 on Apple
Silicon) for day-to-day work, so that code runs in Linux, not on your Mac. In
VS Code, run **Dev Containers: Reopen in Container** and pick **ThreeStrands
development**. It installs the locked dependencies and Playwright's Chromium,
and keeps `node_modules` and both Cargo `target` directories in named volumes
so they never mix with macOS build output.

Use it for `pnpm dev`, `pnpm lint`, `pnpm test`, `pnpm test:e2e`, `pnpm build`,
`cargo test`, `cargo clippy`, and rust-analyzer. It cannot build or run the
macOS app, and code under `#[cfg(target_os = "macos")]` is not compiled there;
`pnpm tauri dev` and macOS builds run on the host or in the
[macOS VM](#macos-vm-tart). The
**ThreeStrands Linux build** container remains the x86-64 release environment.

The container is a boundary for your files and keychain, not for the
repository: the checkout is mounted read-write, so anything that runs inside
can edit sources or `.git/hooks`. VS Code also forwards your Git credentials
and SSH agent into the container by default. Review diffs before committing
from the host.

### macOS VM (Tart)

To build and run the macOS app without dependency code on your Mac, use a
[Tart](https://tart.run) VM. Tart is free for personal use and small
organizations; check its [licensing](https://tart.run/licensing/) for yours.

Install Tart from its [GitHub release](https://github.com/cirruslabs/tart/releases)
(the `cirruslabs/cli` Homebrew tap is out of date and fails to install).
Put `tart.app` in `~/Applications` and run it through
`~/Applications/tart.app/Contents/MacOS/tart`, for example with a `tart`
wrapper script on your `PATH`. Then:

```sh
scripts/tart/dev-vm.sh create          # download the Xcode 26 image, install the toolchain
scripts/tart/dev-vm.sh ssh -t passwd   # replace the image's admin/admin password
```

The image is a 69 GB download with a 140 GB virtual disk. The disk is sparse,
so it takes only the space the VM writes, but Rust build output grows quickly;
run `cargo clean` in the VM when it fills. `create` gives the VM 6 CPUs and
16 GB of memory; set `THREESTRANDS_VM_CPUS` or `THREESTRANDS_VM_MEMORY_MB` to
change that, or `THREESTRANDS_VM_DISK_GB` to grow the disk.

The VM mounts the primary checkout **read-only** and works in its own clone at
`~/threestrands`, whose `origin` is that mount. It holds no Git or forge
credentials, so its code reaches the host and the server only through you:

```sh
scripts/tart/dev-vm.sh start   # boot in a window; --no-graphics for headless
scripts/tart/dev-vm.sh ssh     # then, in the VM:
cd ~/threestrands && git fetch && git switch <branch>
pnpm tauri build --debug --bundles app --config '{"bundle":{"macOS":{"signingIdentity":"-"}}}'
open src-tauri/target/debug/bundle/macos/ThreeStrands.app
```

The `--config` override signs ad hoc, because the VM has no Developer ID
certificate. `--debug` skips the [Gmail OAuth credentials](#gmail-oauth-credentials)
that release builds require; set them in the VM shell to build without it.
`pnpm tauri dev`, `pnpm test`, and `cargo test` run there as usual. VS Code's Remote - SSH extension can open `~/threestrands` in the VM,
using `scripts/tart/dev-vm.sh ip` and the `~/.ssh/threestrands-tart` key.

To build from the host instead, commit your work and run:

```sh
scripts/tart/dev-vm.sh build                    # current branch, debug
scripts/tart/dev-vm.sh build --branch master --release --sign
```

`build` starts the VM headless if it is stopped, checks the branch out in a
separate VM worktree (`~/threestrands-build`, so `~/threestrands` is left
alone), builds with ad hoc signing, and copies the app to
`artifacts/macos-tart/`. `--sign` then runs `sign-app`. The VM sees only
commits, so uncommitted changes are not built. `--release` needs the
[Gmail OAuth credentials](#gmail-oauth-credentials) set in your host shell;
they reach the VM over SSH stdin, not a command line.

Back on the host:

```sh
scripts/tart/dev-vm.sh fetch      # VM branches → refs/remotes/tart/*; review before merging
scripts/tart/dev-vm.sh pull-app   # copy the debug app (or --release) to artifacts/macos-tart/
scripts/tart/dev-vm.sh sign-app   # sign that copy with the Developer ID identity
```

Keep the Developer ID key out of the VM: any dependency code that runs there
could read it. `sign-app` signs the pulled copy on the host instead, with the
hardened runtime and a timestamp, as Tauri does. `codesign` only hashes the
bundle and runs nothing from it. Notarized, updater-signed releases still come
from the release workflow.

`fetch` only copies Git objects; nothing from the VM runs on the host. Running
the pulled app on the host does run dependency code, and that build uses the
installed app's identifier (`app.threestrands.mail`), so it opens your real
mailbox, settings, and keychain items. Test inside the VM unless you mean to
do that.

### npm

`pnpm-workspace.yaml` adds to pnpm's defaults, which already refuse dependency
install scripts that are not explicitly allowed:

- `minimumReleaseAge: 10080` — new versions are not installed until they have
  been public for 7 days. Most compromised releases are removed within that
  window. To take an urgent fix sooner, add that exact version to
  `minimumReleaseAgeExclude`. This covers pnpm itself: keep `PNPM_VERSION`
  in `.devcontainer/Dockerfile` equal to `packageManager` in `package.json`,
  or the container's pnpm will try to switch to a version the rule may still
  hold back.
- `trustPolicy: no-downgrade` — installs fail when a package version has
  weaker publishing evidence (trusted publisher, provenance) than an earlier
  release, a common sign of a hijacked maintainer account.
  `trustPolicyExclude` lists reviewed exceptions, such as `semver@6.3.1`, a
  legitimate 6.x backport published after provenance-signed 7.x releases.

### Rust

Both Cargo workspaces (the root `crates/` workspace and `src-tauri`) are
checked with [`cargo vet`](https://mozilla.github.io/cargo-vet/) and
[`cargo deny`](https://embarkstudios.github.io/cargo-deny/). Both tools are
installed in the development container.

```sh
cargo vet                                      # root workspace
(cd src-tauri && cargo vet)
cargo deny check advisories sources            # uses deny.toml
cargo deny --manifest-path src-tauri/Cargo.toml check advisories sources
```

`cargo vet` imports audits published by Mozilla, Google, the Bytecode
Alliance, and others (`supply-chain/` and `src-tauri/supply-chain/`). Crates
locked before it was adopted are listed as exemptions. Any new crate or
version must be covered by an imported audit, audited with `cargo vet
certify`, or deliberately exempted, so dependency changes show up in review.
After a dependency change, run `cargo vet` in the affected workspace; `cargo
vet suggest` lists the smallest diffs to review. `cargo vet prune` drops
exemptions that imported audits now cover.

`cargo deny` fails on RustSec security advisories, yanked crates, and
dependencies from any source other than crates.io.

The **Supply chain** workflow (`.github/workflows/supply-chain.yml`) runs all
four checks on pull requests that touch Cargo manifests, lockfiles, or audit
files, on every push to `master`, and weekly, since new advisories can fail
an unchanged lockfile. It installs pinned, checksum-verified release
binaries; keep their versions in step with `CARGO_VET_VERSION` and
`CARGO_DENY_VERSION` in `.devcontainer/Dockerfile`.

## Releases

Commit the release changes on `master`, then run:

```sh
pnpm release
```

This reads the version from `package.json`, checks that it matches the Tauri
configuration, Cargo manifest, and application entry in `src-tauri/Cargo.lock`,
then stamps the `## Unreleased` entries in `CHANGELOG.md` with the version and
today's date and commits them (skipped when there are none), pushes `master`,
creates a signed `v<version>` tag, and pushes that tag.
It requires a clean working tree and your configured Git signing key, and stops
if any Git command fails. For example, version `0.56.0` produces tag `v0.56.0`;
version `0.57.0-beta.1` produces a beta prerelease tag.

Before stamping, it warns about bold UI names in the unreleased **New** and
**Improved** entries that appear nowhere in `site/docs` or `site/index.html`,
then releases anyway. Run `pnpm release:docs` to see that list before
releasing. Update the docs, or add names the site words differently to
`DOCS_CHECK_IGNORED` in `scripts/release.mjs`.

The release workflow builds installers and attaches them to a draft GitHub
Release for installation checks before publishing. If the final tag push fails,
retry `git push origin v<version>`; the signed tag already exists locally.

Only the macOS builds are required. If the Linux or Windows build fails, the
draft is still created without that platform's installers and without its entry
in `latest.json`, and the run summary names what is missing. Copies on that
platform keep their current version until a later release includes it. To fill
the gap before publishing, re-run the failed job and then the release job.

### Release notes and the changelog

`CHANGELOG.md` is the only source of release notes. The draft release's notes
combine every changelog section newer than the last *published* release (drafts
never shipped, so they don't count), grouped as New, Improved, and Fixed, with a
link to the code changes. The marketing site's `changelog.html` shows the same
released sections; `## Unreleased` entries stay off the site until a release
stamps them.

If a release build fails before its draft is published, fix it on `master`,
then move the tag instead of bumping the version:

```sh
git tag -d v<version> && git push origin :refs/tags/v<version>
pnpm release
```

Entries added since the first attempt merge into that version's section. Once a
release is published, always ship fixes under a new version.

### Automatic updates

Installed copies check
`https://github.com/oa-labs/ThreeStrands/releases/latest/download/latest.json`
at launch and every six hours, and install only after the user chooses
**Install and restart**. GitHub's "latest" release excludes drafts and
prereleases, so publishing a stable draft is what offers it as an update; betas
are never offered. The Mac app and the AppImage replace themselves. Mac copies
still running from the disk image, and deb and rpm installs, are linked to the
release page instead. Development builds never check.

Every update is signed with a Tauri updater key, separate from the Apple
certificate. The app trusts only the public key in `src-tauri/tauri.conf.json`
(`plugins.updater.pubkey`), and `pnpm release` refuses to tag without it. To
set it up once:

```sh
pnpm tauri signer generate -w ~/.tauri/threestrands-updater.key
```

Put the contents of `~/.tauri/threestrands-updater.key.pub` in
`plugins.updater.pubkey`, and add the private key file's contents and its
password as the `TAURI_SIGNING_PRIVATE_KEY` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` repository secrets. Keep an offline backup
of the private key and password. If they are lost, installed copies can never
be updated again and every user must reinstall by hand.

Release builds pass `src-tauri/tauri.updater.conf.json`, which makes Tauri sign
the update packages; local builds don't, so they need no key. Before
publishing, the release job verifies every signature against the configured
public key, so a mismatched secret fails the release instead of users' updates.

## Marketing screenshots

The browser preview can run against a fictional showcase mailbox instead of
the test fixtures. It has two accounts, multi-message threads, calendar,
tasks, and contacts, and every address is on a reserved `.example` domain:

```sh
VITE_DEMO_DATASET=showcase pnpm dev  # explore it by hand
pnpm screenshots                     # capture every scene, light and dark
```

`pnpm screenshots` writes 2x PNGs to `artifacts/screenshots/<theme>/` with the
clock frozen to a Tuesday morning. Set `SCREENSHOT_DIR` or `SCREENSHOT_NOW` to
override either. `pnpm site:images` honors the same `SCREENSHOT_DIR`, and validates
the complete light/dark capture set before replacing assets. Scenes live in
`tests/screenshots/marketing.spec.ts`; data and deterministic AI responses live
in `src/data/showcaseDataset.ts`.
The brief, thread-chat, calendar-creation, newsletter, Trust, Inbox zero, and
signature scenes use fictional inputs. AI captures configure an in-memory
placeholder key and never call a provider. The Files capture supplies the
bundled rollout image as the native preview response because Quick Look is
unavailable in the browser; it does not change the app or demo client.
Hands-on visual review remains the developer’s responsibility. Keep the default
fixtures in `src/data/demoDataset.ts` unchanged, because the test suites depend
on them.

## Website

The marketing site is a single static page in `site/`, built with Vite and no
framework. It self-hosts its fonts and makes no third-party requests.

```sh
pnpm site:dev     # local preview at http://localhost:1423
pnpm test:site    # build it, then run its Playwright checks
pnpm site:images  # after `pnpm screenshots`: refresh the committed AVIF/WebP set
pnpm site:og      # re-render the social card (site/public/og.png)
```

`.github/workflows/site.yml` deploys to GitHub Pages when `site/` changes on
`master`. The production build uses relative asset URLs so the same output
works at both the custom-domain root and `/ThreeStrands/` on GitHub Pages.
The site tests cover both hosting paths.

## Gmail OAuth credentials

To connect Gmail, enable the Gmail API in a Google Cloud project, create an
OAuth client of type **Desktop app**, and launch the native client with its
client ID and client-secret value:

```sh
THREESTRANDS_GOOGLE_CLIENT_ID="1234.apps.googleusercontent.com" \
THREESTRANDS_GOOGLE_CLIENT_SECRET="value-from-downloaded-desktop-client-json" \
pnpm tauri dev
```

Release builds require these same two variables and fail at build time when
either is missing, so an installed build cannot silently lose Gmail access.

Google labels this value a client secret, but installed desktop applications
are public clients and cannot keep it confidential. It is used only as a
required token-endpoint parameter; OAuth tokens are stored in the
operating-system keychain. Never commit the credential value.

Sign-in opens the system browser and listens on a random loopback port, so
both must be available. Token storage needs a logged-in desktop keychain
service.

## Live-account release checks

Automated coverage includes native SQLite restart and migration tests, provider
contract suites, fault injection, MIME round trips, controlled send clocks and
transports, and Playwright keyboard, compose, and layout workflows. Checks
against real accounts are left to the developer and remain release gates until
recorded as done:

1. Connect a Google test account and confirm OAuth, initial import, and
   incremental sync.
2. Compose and send only to a designated test recipient; inspect received
   To/Cc/Bcc behavior and message text.
3. Reply and reply-all; confirm Gmail threading, including edited subjects.
4. Forward and compare attachment bytes and Unicode filenames at receipt.
5. Save offline, quit and relaunch, reconnect, send, and undo.
6. Exercise file-picker cancel, window close and app quit, token expiry, and
   account disconnect/reconnect while mail is queued.
7. Verify a scheduled send, sleep/wake and restart recovery, and read-only
   visibility on a second enrolled computer.

If a delivery outcome is unresolved, check the provider's Sent folder before
composing another message.


## Maintaining user help

User instructions live in `site/docs/`. When shipping a user-visible change, check the affected guide, update its `reviewed` version only after reviewing the content, and run `pnpm test:site`. Preserve explicit heading IDs when renaming sections; keep old section links when moving content. The shortcut reference and transferred-settings list are generated from app catalogs. Do not recreate a second feature inventory in repository docs.
