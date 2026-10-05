# ThreeStrands

ThreeStrands is a source-available, keyboard-first, local-first email client inspired
by some of the best email clients available. It is designed for people who
spend significant time in email and want to process important conversations
quickly without losing follow-ups, context, or control of their data.

The core experience combines:

- instant keyboard navigation and a discoverable command palette;
- focused inbox triage, local full-text search, and optimistic actions;
- reliable archive, read, star, label, and eventual follow-up workflows;
- offline access backed by a local SQLite cache; and
- a privacy-conscious desktop architecture that connects directly to mail
  providers without requiring a ThreeStrands backend in the free, signed-out mode.

ThreeStrands currently targets Gmail through a standalone Tauri desktop
application. OAuth credentials are kept in the operating-system keychain,
message data and search indexes stay in local SQLite, and a durable mutation
queue reconciles local actions with Gmail.

The detailed product definition, architecture, and delivery roadmap are in
[`PLAN.MD`](PLAN.MD).

## Status

Read/triage and the Phase 2 correspondence workflow are implemented, with live-account validation still required. Compose, reply, forward, local drafts, attachments, and a durable undo-send outbox are available. See [correspondence status and shortcuts](docs/phase-2-status.md).

The browser development build runs against deterministic fixtures. The native
Tauri build uses local SQLite through the same typed client interface. See
[`docs/phase-1.md`](docs/phase-1.md) for the implemented and remaining scope.

## Development

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

### Releases

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

### Marketing screenshots

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

### Website

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

## Product principles

1. Every common action is available from the keyboard.
2. The interface responds immediately, including on unreliable connections.
3. Users can see and control what is synchronized or sent to optional services.
4. Provider-specific behavior is isolated behind a tested capability contract.
5. The desktop client connects directly to mail providers and requires no
   project-operated backend. Optional, end-to-end encrypted cross-device sync
   replicates application-owned workflow data through storage the user
   chooses.

Cross-device sync is documented in
[`docs/cross-device-sync.md`](docs/cross-device-sync.md). It never carries
mail, provider credentials, or AI keys.

## Independent implementation

ThreeStrands uses its own name, visual identity, product language, and
implementation. It learns from established interaction patterns across the
email category without copying proprietary code, assets, or trademarks.

## License

Copyright © 2026 OpenArc LLC. ThreeStrands is source available under the
[PolyForm Perimeter License 1.0.1](LICENSE).

You can inspect, build, use, and modify the software for permitted purposes,
including personal and internal business use. Copying and redistribution are
allowed subject to the license's conditions, including keeping the license and
required notices. Providing others with a competing product built from this
software is prohibited, even if that product is free.

The source is available so you can inspect how ThreeStrands handles your mail
and credentials. This is a source-available license with a noncompete restriction,
rather than an open-source license. The full text in [LICENSE](LICENSE) controls;
third-party dependencies remain subject to their own licenses. OpenArc LLC may
offer separate licenses for uses outside these terms.
