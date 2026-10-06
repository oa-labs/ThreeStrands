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
[`docs/phase-1.md`](docs/phase-1.md) for the implemented and remaining scope and
[`docs/phase-2-status.md`](docs/phase-2-status.md) for correspondence status.

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
then pushes `master`, creates a signed `v<version>` tag, and pushes that tag.
It requires a clean working tree and your configured Git signing key, and stops
if any Git command fails. For example, version `0.56.0` produces tag `v0.56.0`;
version `0.57.0-beta.1` produces a beta prerelease tag.

The release workflow builds installers and attaches them to a draft GitHub
Release for installation checks before publishing. If the final tag push fails,
retry `git push origin v<version>`; the signed tag already exists locally.

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
The brief, thread-chat, and calendar-creation scenes use fictional inputs; AI
captures configure an in-memory placeholder key and never call a provider.
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
