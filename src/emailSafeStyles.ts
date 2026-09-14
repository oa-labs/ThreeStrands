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
  "text-decoration": /^(none|underline|line-through)( (underline|line-through))?$/,
  "vertical-align": /^(baseline|top|middle|bottom|sub|super|text-top|text-bottom)$/,
  "border-collapse": /^(collapse|separate)$/,
  "color": safeColor,
  "background-color": safeColor,
  "line-height": /^(normal|\d+(\.\d+)?(px|%)?)$/,
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
};

export const backgroundImageUrl = /^url\((?:"([^"]*)"|'([^']*)'|([^'")]*))\)$/i;
export const safeImageSrc = /^(https?:|data:image\/)/i;
export const blockedSrcAttr = "data-blocked-src";
