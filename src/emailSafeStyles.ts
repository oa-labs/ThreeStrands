// The single allowlist shared by SafeMessage.tsx's inline `style=""` handling
// and emailStyleSheet.ts's `<style>` block handling — one set of safe
// properties/values, checked the same way regardless of where a sender put
// the CSS. Keep it here rather than duplicated in either caller.

// A CSS color value, shared by color/background-color/bgcolor/border colors.
// None of these forms can carry a network request or executable code.
const colorValue = "(#[0-9a-f]{3,8}|rgba?\\([\\d.\\s,%]+\\)|hsla?\\([\\d.\\s,%]+\\)|transparent|currentcolor|[a-z]+)";
export const safeColor = new RegExp(`^${colorValue}$`, "i");
export const safeBorder = new RegExp(
  `^\\d+(?:\\.\\d+)?px (?:none|solid|dashed|dotted|double|groove|ridge|inset|outset) ${colorValue}$`,
  "i",
);
// offset-x offset-y blur-radius [spread-radius] color, with an optional
// leading "inset" — the same shape safeBorder already validates a color
// against, just with up to four lengths ahead of it instead of one.
export const safeBoxShadow = new RegExp(
  `^(inset )?-?\\d{1,3}(?:\\.\\d+)?px -?\\d{1,3}(?:\\.\\d+)?px \\d{1,3}(?:\\.\\d+)?px(?: \\d{1,3}(?:\\.\\d+)?px)? ${colorValue}$`,
  "i",
);

// A sender's font stack affects more than the face itself: its ascent and
// descent metrics determine where text sits inside the border-built buttons
// common in HTML email. Keep ordinary local/generic family lists, while
// excluding CSS functions, escapes, and other syntax that could refer to a
// value outside this declaration. Embedded/remote fonts remain impossible
// because @font-face is not admitted by the stylesheet sanitizer or CSP.
const safeFontFamily = /^(?=.{1,200}$)[a-z0-9 _,'"-]+$/i;

// A bounded CSS length: 0-4 digit px or percent, matching the same shape
// safeDimension/safeCssLength in SafeMessage.tsx already enforce for the
// width/height *attributes* dimensionAttributeTags carries. Redefined here
// as a plain regex (rather than imported) since this module is imported by
// SafeMessage.tsx, and a value-shape check like this can't carry a
// network request or positioning regardless of which element it lands on.
const safeDimensionValue = /^\d{1,4}(?:\.\d+)?(?:px|%)$/;
const safeMaxDimensionValue = /^(\d{1,4}(\.\d+)?(px|%)|none)$/;

// Keep text formatting without allowing positioning or CSS network requests.
// color/background-color/border-color are safe to keep as-is since they
// never carry a network request; senders that set light text without a
// matching background are rare in practice and this is a legibility
// tradeoff, not a security one. background-image is handled separately
// (gated the same way as <img src>) since it needs URL validation and
// remote-image resolution that a bare property/value allowlist can't do.
export const safeStyles: Record<string, RegExp> = {
  "text-align": /^(left|right|center|justify|start|end)$/,
  "font-weight": /^(normal|bold|[1-9]00)$/,
  "font-style": /^(normal|italic|oblique)$/,
  "font-family": safeFontFamily,
  "text-decoration": /^(none|underline|line-through)( (underline|line-through))?$/,
  "vertical-align": /^(baseline|top|middle|bottom|sub|super|text-top|text-bottom)$/,
  "border-collapse": /^(collapse|separate)$/,
  "color": safeColor,
  "background-color": safeColor,
  // Bounded to 3 digits (max 999) — generous for any real heading/label/price
  // size, but finite. Senders very commonly size headings/labels in em/rem
  // rather than px (this property was previously missing from the
  // allowlist entirely, so every explicit size — a 1.5em heading, a 12px
  // label, a 36px price — was silently dropped and fell back to one
  // uniform body size, flattening the sender's intended visual hierarchy).
  "font-size": /^\d{1,3}(\.\d+)?(px|em|rem|%)$/,
  // em/rem added alongside the pre-existing px/% support for the same
  // reason as font-size above — senders pair a line-height in em with a
  // font-size in em (e.g. "font-size:1.5em;line-height:1.3em") and having
  // only one of the pair survive throws off the intended spacing.
  "line-height": /^(normal|\d+(\.\d+)?(px|%|em|rem)?)$/,
  // Marketing templates (MJML-generated ones especially) commonly size a
  // layout column with an inline `width` percentage, and cap a product
  // image with `max-width`/`max-height` alongside a `width: 100%` that's
  // meant to only fill up to that cap. Without these, a sender's own size
  // constraint is silently dropped while the `width: 100%` that was meant
  // to pair with it survives, leaving the image (or column) free to grow
  // to the full width of its container instead of its intended thumbnail
  // size.
  "width": safeDimensionValue,
  "max-width": safeMaxDimensionValue,
  "max-height": safeMaxDimensionValue,
  "border": safeBorder,
  "border-top": safeBorder,
  "border-right": safeBorder,
  "border-bottom": safeBorder,
  "border-left": safeBorder,
  // Marketing ESPs (Customer.io, Klaviyo, HubSpot, Mailchimp) widely pair a
  // black background with a screen+difference blend-mode stack to defeat
  // Gmail's automatic dark-mode color inversion: both blend modes are a
  // no-op against black, so the pair cancels out to fully transparent in
  // any renderer that honors mix-blend-mode. Without it, the black
  // background has nothing to cancel it and renders as an opaque block.
  "mix-blend-mode": /^(normal|multiply|screen|overlay|darken|lighten|color-dodge|color-burn|hard-light|soft-light|difference|exclusion|hue|saturation|color|luminosity)$/,
  // The other half of ESP light/dark template support: a `<style>` block
  // toggling which of two logo/icon variants is visible under a
  // `prefers-color-scheme` media query (see emailStyleSheet.ts). Neither
  // value carries positioning or a network request.
  "display": /^(none|block|inline|inline-block|table|table-cell|table-row|inline-table|flex|inline-flex)$/,
  "visibility": /^(visible|hidden|collapse)$/,
  // Purely cosmetic text/box formatting: none of these can carry a network
  // request, reposition an element outside its box, or escape the iframe's
  // own layout constraints.
  "border-radius": /^\d{1,4}(?:\.\d+)?(?:px|%)(?:\s+\d{1,4}(?:\.\d+)?(?:px|%)){0,3}$/,
  "box-shadow": safeBoxShadow,
  "text-transform": /^(none|uppercase|lowercase|capitalize)$/,
  "letter-spacing": /^(normal|-?\d{1,3}(?:\.\d+)?px)$/,
  "white-space": /^(normal|nowrap|pre|pre-wrap|pre-line)$/,
  "word-break": /^(normal|break-all|keep-all|break-word)$/,
  "border-spacing": /^\d{1,4}(?:\.\d+)?px(?:\s+\d{1,4}(?:\.\d+)?px)?$/,
};

export const backgroundImageUrl = /^url\((?:"([^"]*)"|'([^']*)'|([^'")]*))\)$/i;
export const safeImageSrc = /^(https?:|cid:|data:image\/)/i;
export const blockedSrcAttr = "data-blocked-src";
