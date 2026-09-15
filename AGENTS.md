# Agent guidance

## Email rendering

Dispatch renders untrusted sender HTML inside a sandboxed, CSP-scoped iframe. Keep that trust boundary intact, but treat safe sender-authored structure as the source of truth.

- Do not remove markup solely because it is empty, whitespace-only, or visually redundant. Sanitization removes unsafe capabilities, not layout structure.
- Never branch on a sender, domain, brand, provider class, provider ID, or one captured template. A compatibility fix must express a provider-neutral rendering or security invariant and include at least two structurally different fixtures.
- Add CSS properties by complete capability family (typography, box model, table, flex/flow, visibility, or cosmetic). Document why the family cannot fetch resources, execute code, escape the iframe, or create an unsafe interaction.
- Put every numeric limit in `src/emailRenderingPolicy.ts`. Use the shared parser and test below-limit, exact-limit, and above-limit behavior. Do not silently clamp values in component code.
- Keep iframe fallback CSS low-specificity and non-invasive. It must not override author CSS or HTML presentational attributes such as table cell spacing.
- Quote folding must use semantic, conservative evidence. Provider-specific selectors are prohibited in production quote detection; uncertain quoted content stays visible.
- Any remote image or background URL must remain behind the existing native proxy and blocked-source flow. Do not admit CSS URLs through a new path.
- Rendering changes require security-negative tests and visual or fixture-based regression coverage. If a provider-specific exception appears unavoidable, stop and document the invariant and review need instead of adding it silently.
