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
