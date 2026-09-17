# Agent guidance

## Testing

Agents are responsible for the automated test suite: run it (`pnpm test`, `pnpm test:e2e`), add tests that cover the change being made, and maintain existing tests (fix or update them when behavior intentionally changes, don't just delete or skip a failing test to get green).

Agents are not responsible for manually exercising the running application (starting the dev server, clicking through the UI, taking screenshots) to confirm a fix looks or feels right — that verification is done by the developer. Ship the code change backed by automated coverage and let the developer do the hands-on check.

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
