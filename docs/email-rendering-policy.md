# Email rendering policy

Dispatch treats every message body as untrusted HTML. `SafeMessage` sanitizes structure and capabilities, resolves remote images through the native proxy, then renders the result in a sandboxed iframe with a restrictive CSP. Links are intercepted and opened by the native opener.

The renderer is sender-fidelity-first inside that boundary. Safe layout is preserved; the sanitizer does not delete empty elements or rewrite table structure merely to impose Dispatch spacing. Fallback CSS is deliberately low-specificity so inline sender CSS and HTML presentational attributes win.

All inline declarations, embedded stylesheet declarations, and HTML dimensions use `src/emailRenderingPolicy.ts`. The policy accepts bounded, finite presentation values and rejects values outside the shared limits rather than rewriting them. External resources, scripts, forms, generated content, animations, viewport overlays, and unproxied `url(...)` values remain forbidden.

Embedded stylesheets are parsed with PostCSS and a selector parser. Retained selectors are scoped under `[data-email-root]`; `html`, `body`, and `:root` are rewritten to that root. Safe responsive media queries may use `screen`/`all`, bounded width features, orientation, and the reader-selected color scheme. Unsupported at-rules are removed.

Quoted-history folding is independent of provider markup. It scores standalone reply separators, reply-introduction lines, trailing blockquotes/citations, and compact message-header clusters. Folding requires strong combined evidence; ambiguous content remains visible and can be expanded when folded.

When changing this policy, add structurally distinct fixtures, security-negative cases, numeric-boundary tests, and visual coverage at narrow/wide widths and light/dark themes. Do not encode a sender-specific workaround in production logic.
