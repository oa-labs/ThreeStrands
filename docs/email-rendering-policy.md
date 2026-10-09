# Email rendering policy

ThreeStrands treats every message body as untrusted HTML. `SafeMessage` sanitizes structure and capabilities, resolves remote images through the native proxy, then renders the result in a sandboxed iframe with a restrictive CSP. Links are intercepted and opened by the native opener.

The pipeline has deliberately separate stages: structural sanitization removes unsafe nodes and attributes while preserving harmless structure; CSS sanitization applies the shared typed policy; remote-resource gating parks image URLs until the proxy resolves them; quote folding selects only high-confidence semantic boundaries; isolated rendering applies the iframe CSP and document-height containment. A stage may enforce its boundary, but does not perform visual cleanup belonging to another stage.

The renderer is sender-fidelity-first inside that boundary. Safe layout is preserved; the sanitizer does not delete empty elements or rewrite table structure merely to impose ThreeStrands spacing. Fallback CSS is deliberately low-specificity so inline sender CSS and HTML presentational attributes win.

All inline declarations, embedded stylesheet declarations, and HTML dimensions use `src/emailRenderingPolicy.ts`. The policy accepts bounded, finite presentation values and rejects values outside the shared limits rather than rewriting them: absolute lengths are capped at 4096px, relative lengths at 64em/rem, percentages at 100%, font sizes at 256px/16em/rem/1600%, unitless line-height and flex factors at 16, opacity at 1, and the isolated frame at 50,000px. External resources, scripts, forms, generated content, animations, viewport overlays, and unproxied `url(...)` values remain forbidden.

The allowlisted CSS capability families are typography, box model, table layout, flex layout, flow, visibility, and cosmetic effects. They are admitted as complete safe families so a button or table does not become a partial layout. Embedded stylesheets are parsed with PostCSS and a selector parser. Retained selectors are scoped under `[data-email-root]`; `html`, `body`, and `:root` are rewritten to that root. Safe responsive media queries may use `screen`/`all`, bounded width features, orientation, and the reader-selected color scheme. Unsupported at-rules are removed.

Quoted-history folding is independent of provider markup and lives in `src/quotedHistory.ts`. It runs on the already sanitized (and linkified) HTML, which it first flattens into reader-visible lines: block elements and `<br>` start lines, inline elements such as links and bold labels do not, and source-formatting whitespace is ignored. Matching lines rather than individual text nodes is what lets an attribution like `On …, A. Sender <sender@example.com> wrote:` survive its address becoming a link.

Evidence and weights:

- A standalone reply separator or reply-introduction line (`Original message`, `Forwarded message`, `On … wrote:`, including one whose `wrote:` hard-wrapped onto a following line): 3.
- A From/Sent/To/Subject header cluster — consecutive non-blank lines that include `From:`, at least three distinct fields, and an address or timestamp: 2, or 3 when it is complete (four or more fields including `Sent:` or `Date:`). A horizontal rule or separator line directly above the cluster adds 1, and the fold then starts at that rule.
- A trailing blockquote or citation with meaningful quoted prose (nothing meaningful after it): 2. Empty, whitespace-only, hidden-only, image-only, or attribution-only regions stay visible and contribute no quote evidence.
- A trailing run of `>`-prefixed lines (at least `minQuoteRunLines`, with only blank lines between them and the end): 3. Quoted lines followed by unquoted text are an inline reply and never count.
- An attribution paired with a quoted region that follows it, or that opens it as the region's first line: +2 for each.
- Non-empty current content before the boundary: 1.

Repeated thread text is fill-in evidence, used only when the reader renders a message inside its conversation. `src/threadTextIndex.ts` indexes each message's plain text (decoded, or flattened from HTML in an inert document) as `shingleWords`-word shingles after removing quote markers, case and punctuation, and a message sees only shingles from messages before it. Shingles run across line breaks, so short and rewrapped lines still match. A line counts as repeated when matched shingles cover at least `minSeenLineCoverage` of its words; lines without words are neutral. Walking up from a boundary, the contiguous run of repeated or neutral lines:

- extends a structural fold upward when it holds at least `minCorroboratingShingles` matches — typically the sender's signature repeated above the quote;
- confirms a lone trailing blockquote or citation with the same minimum of matches inside the quoted prose itself; adjacent repeated boilerplate cannot supply that evidence.

Repetition alone never establishes quoted history, in HTML or plain text. Reports, notifications, addresses, and unsubscribe notices can repeat across a conversation while still being current content. Folding requires semantic quote evidence; a matching footer must stay intact rather than being cut midway through its layout. Repeated text followed by new text never folds, a signature the conversation has not shown before stays visible, and an extension that would leave nothing visible falls back to the structural fold. Text a message shares with no earlier message, such as a genuine quotation, gets no repeated-text evidence.

Attributions and header clusters also require meaningful quoted prose after them; bare markers and empty quote prefixes do not fold. Hidden markup remains in the sanitized message but its text is excluded from quote detection. Repeated-text extensions may remove whole blocks, but cannot cut inside table or flex layout or introduce a partial wrapper that the original quote boundary did not split. If extending would split a layout, keep the original quote boundary and leave the repeated content visible. This also protects unfamiliar wrappers styled through sender classes without interpreting provider-specific selectors.

Folding requires `foldScoreThreshold` (four) points. When the boundary line begins one or more wrapper elements, the cut moves before the outermost wrapper so the visible copy does not end in an empty blockquote or rule. A lone trailing blockquote (3) and a mid-message header cluster without a rule or complete fields (3) stay visible. Ambiguous content remains visible and can be expanded when folded. All folding limits are in `EMAIL_QUOTE_FOLDING_LIMITS`. This is visual normalization, not a security decision: the security stages still sanitize and contain the complete message.

The reader's fold toggle stays outside the message document. After sanitization, ThreeStrands inserts one empty `span[data-quoted-history-fold]` marker where the visible part ends, in both the folded and expanded documents. The iframe stylesheet gives that span a fixed height to make room, and the parent places the toggle over it. The toggle therefore stays in the same place when the quoted part is shown and can fold it again. Sanitization drops sender data attributes, and folding removes any existing marker attribute before inserting its own, so sender markup cannot supply or move the marker.

When changing this policy, add structurally distinct fixtures, security-negative cases, numeric-boundary tests, and visual coverage at narrow/wide widths and light/dark themes. Do not encode a sender-specific workaround in production logic.

## Reader minimum font size

The optional minimum email font size defaults to off. The shared policy accepts
zero (off) or whole CSS-pixel values from 12 through 32. The parent adjusts
computed typography in the already sanitized, CSP-scoped iframe; it adds no
sender CSS capabilities or resource paths. The accessibility override deliberately
raises small text above author sizes, while the existing fallback CSS stays
unchanged. All baseline sizes are read before any writes so relative descendants
and larger headings keep their intended sizes. Explicit line spacing grows to
at least 1.2 times the new font size; existing generous spacing is preserved
without multiplying it by a sender-controlled tiny font size. Empty,
whitespace-only, and zero-font spacers retain their structure.
Changes and responsive resizing restore author styles before recalculation; off
restores author typography. Plain-text emails also respect the floor. Raster text
inside images cannot be enlarged independently, and sender fixed-size boxes may
wrap or overflow when text grows.
