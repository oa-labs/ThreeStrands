# Email rendering policy

ThreeStrands treats every message body as untrusted HTML. `SafeMessage` sanitizes structure and capabilities, resolves remote images through the native proxy, then renders the result in a sandboxed iframe with a restrictive CSP. Links are intercepted and opened by the native opener.

The pipeline has deliberately separate stages: structural sanitization removes unsafe nodes and attributes while preserving harmless structure; CSS sanitization applies the shared typed policy; remote-resource gating parks image URLs until the proxy resolves them; quote folding selects only high-confidence semantic boundaries; isolated rendering applies the iframe CSP and document-height containment. A stage may enforce its boundary, but does not perform visual cleanup belonging to another stage.

The renderer is sender-fidelity-first inside that boundary. Safe layout is preserved; the sanitizer does not delete empty elements or rewrite table structure merely to impose ThreeStrands spacing. Fallback CSS is deliberately low-specificity so inline sender CSS and HTML presentational attributes win.

All inline declarations, embedded stylesheet declarations, and HTML dimensions use `src/emailRenderingPolicy.ts`. The policy accepts bounded, finite presentation values and rejects values outside the shared limits rather than rewriting them: absolute lengths are capped at 4096px, relative lengths at 64em/rem, percentages at 100%, font sizes at 256px/16em/rem/1600%, unitless line-height and flex factors at 16, opacity at 1, and the isolated frame at 50,000px. External resources, scripts, forms, generated content, animations, viewport overlays, and unproxied `url(...)` values remain forbidden.

The allowlisted CSS capability families are typography, box model, table layout, flex layout, flow, visibility, and cosmetic effects. They are admitted as complete safe families so a button or table does not become a partial layout. Embedded stylesheets are parsed with PostCSS and a selector parser. Retained selectors are scoped under `[data-email-root]`; `html`, `body`, and `:root` are rewritten to that root. Safe responsive media queries may use `screen`/`all`, bounded width features, orientation, and the reader-selected color scheme. Unsupported at-rules are removed.

Quoted-history folding is independent of provider markup. It scores standalone reply separators or reply-introduction lines (3), trailing blockquotes/citations (2), compact header clusters with an address or timestamp (2), and non-empty current content (1). Folding requires at least four points; ambiguous content remains visible and can be expanded when folded. This is visual normalization, not a security decision: the security stages still sanitize and contain the complete message.

When changing this policy, add structurally distinct fixtures, security-negative cases, numeric-boundary tests, and visual coverage at narrow/wide widths and light/dark themes. Do not encode a sender-specific workaround in production logic.

## Reader minimum font size

The optional minimum email font size defaults to off. The shared policy accepts
zero (off) or whole CSS-pixel values from 12 through 32. The parent adjusts
computed typography in the already sanitized, CSP-scoped iframe; it adds no
sender CSS capabilities or resource paths. The accessibility override deliberately
raises small text above author sizes, while the existing fallback CSS stays
unchanged. All baseline sizes are read before any writes so relative descendants
and larger headings keep their intended sizes. Explicit line spacing scales with
enlarged text. Empty, whitespace-only, and zero-font spacers retain their structure.
Changes and responsive resizing restore author styles before recalculation; off
restores author typography. Plain-text emails also respect the floor. Raster text
inside images cannot be enlarged independently, and sender fixed-size boxes may
wrap or overflow when text grows.
