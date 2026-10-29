# Agent guidance

## Working tree

Do every task that changes files in its own git worktree on a new branch from
`master` (for example under `../dispatch-worktrees/<branch>`), never in the
developer's primary checkout. Other agents and the developer may be editing the
primary checkout at the same time.

Exception: documentation-only changes and small bug fixes may be made directly
in the primary checkout. Touch only the files the change needs, check the
checkout's status first, and leave any other agent's uncommitted edits alone.
The rules below still apply, including no `git stash`, `checkout`, or `reset`.

- Do not run `git stash`, `git checkout`, `git reset`, or other commands that
  rewrite files in the primary checkout.
- Run tests, the version bump, and changelog edits inside the worktree.
- Merge into the primary checkout only when the developer asks. Before merging,
  check the primary checkout's status and report any files another agent has
  also changed.
- Leave nothing behind. Once the branch is merged, or the developer drops the
  work, remove the worktree (`git worktree remove`) and delete its branch. A
  worktree should exist only while its task is in progress or waiting to be
  merged. Mention any still waiting in the final summary.

## Testing

Agents own the automated test suite. The developer owns looking at the running app.

Treat existing tests as the current product contract.

When a test fails:
1. If this task did not mean to change that behavior, fix the implementation.
2. If this task did change the contract, update the test to the new contract and
   say so in the summary. Do not weaken security, settings-transfer, or
   rendering invariants to make a change easier.
3. Never delete a test, skip it, or comment it out to get green.
4. Add tests for new behavior; prefer extending an existing describe over a
   one-off assertion in an unrelated file.

Test behavior at the lowest level that owns it: a hook with `renderHook`, or a
component rendered directly (see `src/test/settingsHarness.tsx`). Render
`<App />` only to check wiring between panes, shortcuts, workspaces, or
overlays; it is slow and the main source of CI timeouts. Do not fix a slow
test by raising one call's timeout.

Moving a test to a lower level is not deleting it, provided every assertion
lands somewhere. If the App test was the only check that a button, shortcut,
or setting reaches its code, keep a slim App test for that wiring.

Assert on-screen text only when it depends on state, or when it states a
promise (security, data retention, settings transfer). For help text, check
that it is attached with `toHaveAccessibleDescription()` rather than repeating
its wording. Finding elements by role and accessible name is still preferred.

Run `pnpm test` for frontend changes. Run `cargo test` in `src-tauri` for Rust
changes, and `cargo test --workspace` at the repository root when a change
touches `crates/` (mail parsing lives in `crates/mail-mime`). Run `pnpm test:e2e` when the change touches compose, triage, or email
rendering.

Do not start the dev server, click through the UI, take screenshots, or use a
browser to judge look and feel. Ship the change with automated coverage and
leave hands-on UI verification to the developer.

## Settings transfer compatibility

Treat the encrypted settings export as a persistent, cross-version format. Do not remove a field, add a required field, or change a field's meaning under the existing format version without a backward-compatible default or migration. If compatibility cannot be preserved, bump the format version intentionally and keep support for importing earlier versions. Every transfer schema change must include regression coverage for exports produced by the preceding schema.

## Email rendering

ThreeStrands renders untrusted sender HTML inside a sandboxed, CSP-scoped iframe. Keep that trust boundary intact, but treat safe sender-authored structure as the source of truth.

- Do not remove markup solely because it is empty, whitespace-only, or visually redundant. Sanitization removes unsafe capabilities, not layout structure.
- Never branch on a sender, domain, brand, provider class, provider ID, or one captured template. A compatibility fix must express a provider-neutral rendering or security invariant and include at least two structurally different fixtures.
- Add CSS properties by complete capability family (typography, box model, table, flex/flow, visibility, or cosmetic). Document why the family cannot fetch resources, execute code, escape the iframe, or create an unsafe interaction.
- Put every numeric limit in `src/emailRenderingPolicy.ts`. Use the shared parser and test below-limit, exact-limit, and above-limit behavior. Do not silently clamp values in component code.
- Keep iframe fallback CSS low-specificity and non-invasive. It must not override author CSS or HTML presentational attributes such as table cell spacing.
- Quote folding must use semantic, conservative evidence. Provider-specific selectors are prohibited in production quote detection; uncertain quoted content stays visible.
- Any remote image or background URL must remain behind the existing native proxy and blocked-source flow. Do not admit CSS URLs through a new path.
- Rendering changes require security-negative tests and visual or fixture-based regression coverage. If a provider-specific exception appears unavoidable, stop and document the invariant and review need instead of adding it silently.

## Application versioning

Every task that changes the shipped application must include an appropriate
Semantic Versioning bump before completion:

- Patch: bug fixes and small behavior changes.
- Minor: new backward-compatible functionality.
- Major: intentionally incompatible changes.
- Do not bump the version for documentation, tests, CI, or development-only
  changes that do not affect the shipped application.

Keep the application version identical in:

- `package.json`
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml`
- the `threestrands` package entry in `src-tauri/Cargo.lock`

Mention the version change in the final summary.

## Changelog

`CHANGELOG.md` feeds GitHub release notes and the website's changelog page, so
users read it. Whenever a version bump ships a change users can notice, add
an entry under `## Unreleased` in the same task:

- Use `### New`, `### Improved`, or `### Fixed`; create the group if it is
  missing, in that order. Never write a version heading; `pnpm release` stamps
  `## Unreleased` with the version and date.
- Write one or two plain sentences for someone who uses the app, not someone
  who reads the code: what they can now do, or what no longer goes wrong. Name
  features as the UI does (**Settings → Mail Accounts**). No file names,
  components, schema versions, phases, or ticket language.
- Add to an existing entry about the same feature rather than adding a near
  duplicate. Skip changes users cannot notice: refactors, tests, internal
  hardening, and development tooling.
- Run `node --test scripts/changelog.test.mjs` after editing; it rejects any
  other structure.

<!-- entire-agent:begin -->
Read .entire/agent-guide.md for this repository's workflow, source inspection, and verification guidance.
@.entire/agent-guide.md
<!-- entire-agent:end -->
