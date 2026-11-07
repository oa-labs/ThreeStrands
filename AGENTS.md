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
- Run tests and changelog edits inside the worktree.
- Merge into the primary checkout only when the developer asks. Before merging,
  check the primary checkout's status and report any files another agent has
  also changed.
- Leave nothing behind. Once the branch is merged, or the developer drops the
  work, remove the worktree (`git worktree remove`) and delete its branch. A
  worktree should exist only while its task is in progress or waiting to be
  merged. Mention any still waiting in the final summary.

### In the tart VM

You're in the developer's tart VM when `git remote get-url origin` is a path
under `~/Source/.host/`. There, work reaches `master` only when the developer
runs `tart-vm mac land <branch>` on the host. `land` signs copies of the
branch's commits and lands them on the host's `master`. It fast-forwards when
the branch includes `master`, and otherwise makes a signed merge commit if the
two merge without conflicts. Then it removes the branch and its worktree from
the VM. So, in the VM, these rules replace the merge and cleanup rules above:

- Start each task from fresh commits: run `git fetch origin`, then create the
  worktree on a new branch from `origin/master`, not the local `master`.
- Never merge into the VM's `master`, even when the developer asks you to merge
  or says "merge the worktree". Commits merged there never reach the host. When
  the work is committed and ready, tell the developer to run
  `tart-vm mac land <branch>` on the host. Pushing is disabled in the VM, so
  don't try it.
- Leave the worktree and branch in place once the work is ready; `land` removes
  them. Remove them yourself only when the developer drops the work.
- Don't rebase or merge just because the branch is behind `origin/master`;
  `land` merges branches that don't conflict.
- If `land` reports conflicts, run `git fetch origin` in the branch's worktree
  and merge or rebase `origin/master` into it. Resolve the conflicts yourself,
  rerun the tests affected by the conflicting files and by the combined
  changes, and ask the developer to run `land` again. If `land` says some of
  the branch's commits have already landed, rebase it onto `origin/master`,
  which drops them. The developer may need to run `tart-vm mac sync` first, so
  that `origin/master` has their latest commits.
- At the start of a task, after `git fetch origin`, look for leftovers. A branch
  has fully landed when `git cherry origin/master <branch>` prints no `+` lines
  and the branch has commits of its own. Remove its worktree and the branch. Use
  `git worktree remove` without `--force`. If it refuses because of
  uncommitted changes, keep the worktree and tell the developer. The developer
  can also run `tart-vm mac prune` from the host to do the same cleanup.
- In the final summary, list each branch that's waiting to land, along with its
  `land` command.

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

Before committing Rust changes, run `cargo fmt --all` at the repository root
and `cargo fmt --all --manifest-path src-tauri/Cargo.toml`; CI rejects
unformatted code.

Do not start the dev server, click through the UI, take screenshots, or use a
browser to judge look and feel. Ship the change with automated coverage and
leave hands-on UI verification to the developer.

## Dependencies

When a change adds or updates an npm package or Rust crate, follow
[Dependency safety](DEVELOPERS.md#dependency-safety) and run `cargo vet` and
`cargo deny` in the affected workspace. Do not relax `pnpm-workspace.yaml`
policies, add `cargo vet` exemptions or certifications, or ignore advisories
to get a change through; name the package and version in the summary and let
the developer decide.

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

Do not change the application version. `pnpm release` picks it when the
developer cuts a release, from the changelog entries waiting in `changes/`, and
writes it to `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`,
and the `threestrands` entry in `src-tauri/Cargo.lock`. If a change is
intentionally incompatible and needs a major version, say so in the final
summary so the developer can release it with `--major`.

## Changelog

`CHANGELOG.md` feeds GitHub release notes and the website's changelog page, so
users read it. Whenever a task changes the shipped application in a way users
can notice, add a new file in `changes/`, named after the task's branch (for
example `changes/starred-inbox-tab.md`). See `changes/README.md` for the
format. Do not add entries to `CHANGELOG.md` itself.

- Use `### New`, `### Improved`, or `### Fixed`, in that order. The group
  sets the release's version: any **New** entry makes it a minor release, so
  use **New** only for new functionality, not for improvements to an existing
  feature. Never write a `##` heading.
- Write one or two plain sentences for someone who uses the app, not someone
  who reads the code: what they can now do, or what no longer goes wrong. Name
  features as the UI does (**Settings → Mail Accounts**). No file names,
  components, schema versions, phases, or ticket language.
- Add to an existing entry about the same feature rather than adding a near
  duplicate, whether it is in another `changes/` file or under
  `## Unreleased` in `CHANGELOG.md`. Skip changes users cannot notice:
  refactors, tests, internal hardening, and development tooling.
- Run `node --test scripts/changelog.test.mjs` after editing; it rejects any
  other structure.

