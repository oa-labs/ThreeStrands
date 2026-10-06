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

ThreeStrands currently works with Gmail as a desktop application for macOS and
Linux. Sign-in credentials are kept in your operating system's keychain, your
messages and search index stay on your computer, and actions you take offline
are synced to Gmail when you reconnect.

NOTE: Only the macOS app is frequently used/tested currently.

## Install

Download the latest installer from
[GitHub Releases](https://github.com/oa-labs/ThreeStrands/releases/latest):

- **macOS:** the `.dmg` for Apple Silicon (`aarch64`) or Intel (`x64`).
- **Linux (x86-64):** a `.deb`, `.rpm`, or `.AppImage` package.

Launch ThreeStrands and sign in with your Google account to connect Gmail.

## Status

Reading, triage, and correspondence are implemented and still being validated
against live accounts. Compose, reply, forward, local drafts, attachments, and
undo send are available. See the
[correspondence status and keyboard shortcuts](docs/phase-2-status.md).

## Product principles

1. Every common action is available from the keyboard.
2. The interface responds immediately, including on unreliable connections.
3. Users can see and control what is synchronized or sent to optional services.
4. Provider-specific behavior is isolated behind a tested capability contract.
5. The desktop client connects directly to mail providers and requires no
   project-operated backend. Optional, end-to-end encrypted cross-device sync
   replicates application-owned workflow data through storage the user
   chooses.

Cross-device sync is described in
[`docs/cross-device-sync.md`](docs/cross-device-sync.md). It never carries
mail, provider credentials, or AI keys.

## Building from source

To build ThreeStrands yourself or contribute, see [`DEVELOPERS.md`](DEVELOPERS.md).

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
