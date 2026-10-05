/**
 * Shared rendering policy for sender-authored email HTML and CSS.
 *
 * This module is deliberately independent of React and DOMPurify so inline
 * declarations, embedded stylesheets, and legacy HTML dimensions all use the
 * same value grammar and numeric limits.
 */
export const EMAIL_CSS_LIMITS = {
  maxAbsolutePx: 4096,
  maxRelativeLength: 64,
  maxPercentage: 100,
  maxFontSizePx: 256,
  maxFontSizeRelative: 16,
  maxFontSizePercentage: 1600,
  maxLineHeightUnitless: 16,
  // A percentage line-height multiplies the font size, like a unitless one,
  // so it shares that bound (16 × 100%) rather than the layout cap.
  maxLineHeightPercentage: 1600,
  maxUnitlessFactor: 16,
  maxOpacity: 1,
  maxFrameHeightPx: 50_000,
} as const;

/** Reader-owned accessibility preference; zero preserves sender typography. */
export const EMAIL_MINIMUM_FONT_SIZE = {
  disabled: 0,
  minPx: 12,
  maxPx: 32,
  minLineHeightRatio: 1.2,
} as const;

export function parseEmailMinimumFontSize(value: unknown): number {
  return typeof value === "number" && Number.isInteger(value)
    && (value === EMAIL_MINIMUM_FONT_SIZE.disabled
      || (value >= EMAIL_MINIMUM_FONT_SIZE.minPx && value <= EMAIL_MINIMUM_FONT_SIZE.maxPx))
    ? value : EMAIL_MINIMUM_FONT_SIZE.disabled;
}

/**
 * Resource budgets for remote and embedded images. These are kept beside the
 * other sender-controlled rendering limits so components do not invent local
 * caps. The native proxy independently enforces its byte/pixel/cache bounds at
 * the trust boundary; these limits additionally bound fan-out and frontend
 * copies for each rendered message.
 */
export const EMAIL_IMAGE_LIMITS = {
  maxConcurrentGlobally: 4,
  maxPendingGlobally: 40,
  maxConcurrentPerMessage: 2,
  maxImagesPerMessage: 40,
  maxDataUriBytesPerMessage: 20 * 1024 * 1024,
} as const;

/**
 * Bounds for quoted-history folding. Folding is visual normalization only; it
 * never changes what the security stages sanitize or contain.
 */
export const EMAIL_QUOTE_FOLDING_LIMITS = {
  /** Consecutive `>`-prefixed lines needed before a run counts as quoted history. */
  minQuoteRunLines: 5,
  /** Non-blank lines searched for a "wrote:" that hard-wrapped off its opener. */
  wrappedAttributionLookaheadLines: 3,
  /** Longest "On … wrote:" attribution, in characters, including a wrapped one. */
  maxAttributionLength: 500,
  /** Longest From/Sent/To/Subject header cluster, in non-blank lines. */
  maxHeaderClusterLines: 12,
  /** Evidence points a boundary needs, including the point for current content. */
  foldScoreThreshold: 4,
  /** Words per shingle when matching text repeated from earlier messages in the thread. */
  shingleWords: 4,
  /** Fraction of a line's words that matched shingles must cover for the line to count as repeated. */
  minSeenLineCoverage: 0.8,
  /** Matched shingles that let a trailing run of repeated thread text fold on its own. */
  minRepeatedRegionShingles: 8,
  /** Matched shingles that let repeated text extend a structural fold or confirm a lone citation. */
  minCorroboratingShingles: 3,
} as const;

const colorValue ="(#[0-9a-f]{3,8}|rgba?\\([\\d.\\s,%]+\\)|hsla?\\([\\d.\\s,%]+\\)|transparent|currentcolor|[a-z]+)";
export const safeColor = new RegExp(`^${colorValue}$`, "i");

const namedValue = (values: string[]) => new RegExp(`^(?:${values.join("|")})$`, "i");
const cssLength = /^(-?)(\d+(?:\.\d+)?)(px|em|rem|%)$/i;

function boundedLength(value: string, options: { allowNegative?: boolean; allowPercentage?: boolean } = {}): string | null {
  const trimmed = value.trim().toLowerCase();
  if (trimmed === "0") return "0";
  const match = trimmed.match(cssLength);
  if (!match) return null;
  const amount = Number(match[2]);
  const unit = match[3];
  if (!Number.isFinite(amount) || (match[1] === "-" && !options.allowNegative)) return null;
  if (unit === "%") {
    return options.allowPercentage && amount <= EMAIL_CSS_LIMITS.maxPercentage ? trimmed : null;
  }
  const limit = unit === "px" ? EMAIL_CSS_LIMITS.maxAbsolutePx : EMAIL_CSS_LIMITS.maxRelativeLength;
  return amount <= limit ? trimmed : null;
}

function fontSize(value: string): string | null {
  const trimmed = value.trim().toLowerCase();
  const match = trimmed.match(cssLength);
  if (!match || match[1] === "-") return null;
  const amount = Number(match[2]);
  if (!Number.isFinite(amount)) return null;
  const limit = match[3] === "px"
    ? EMAIL_CSS_LIMITS.maxFontSizePx
    : match[3] === "%"
      ? EMAIL_CSS_LIMITS.maxFontSizePercentage
      : EMAIL_CSS_LIMITS.maxFontSizeRelative;
  return amount <= limit ? trimmed : null;
}

function lineHeight(value: string): string | null {
  const trimmed = value.trim().toLowerCase();
  if (trimmed === "normal") return trimmed;
  if (/^\d+(?:\.\d+)?$/.test(trimmed)) {
    return Number(trimmed) <= EMAIL_CSS_LIMITS.maxLineHeightUnitless ? trimmed : null;
  }
  const percentage = trimmed.match(/^(\d+(?:\.\d+)?)%$/);
  if (percentage) return Number(percentage[1]) <= EMAIL_CSS_LIMITS.maxLineHeightPercentage ? trimmed : null;
  return boundedLength(trimmed);
}

function unitlessFactor(value: string): string | null {
  const trimmed = value.trim().toLowerCase();
  if (!/^\d+(?:\.\d+)?$/.test(trimmed)) return null;
  return Number(trimmed) <= EMAIL_CSS_LIMITS.maxUnitlessFactor ? trimmed : null;
}

function opacity(value: string): string | null {
  const trimmed = value.trim().toLowerCase();
  if (!/^\d+(?:\.\d+)?$/.test(trimmed)) return null;
  return Number(trimmed) <= EMAIL_CSS_LIMITS.maxOpacity ? trimmed : null;
}

const safeFontFamily = /^(?=.{1,200}$)[a-z0-9 _,'"-]+$/i;
const safeBorderWidthKeyword = /^(?:thin|medium|thick)$/;
function border(value: string): string | null {
  const trimmed = value.trim().toLowerCase();
  if (/^(?:none|0|0px)$/.test(trimmed)) return trimmed;
  const ordered = trimmed.match(/^(\d+(?:\.\d+)?(?:px|em|rem))\s+(none|solid|dashed|dotted|double|groove|ridge|inset|outset)(?:\s+(.+))?$/);
  if (ordered && (!ordered[3] || safeColor.test(ordered[3]))) return trimmed;
  const parts = trimmed.split(/\s+/);
  if (parts.length > 3) return null;
  const styles = new Set(["none", "solid", "dashed", "dotted", "double", "groove", "ridge", "inset", "outset"]);
  let widthSeen = false;
  let styleSeen = false;
  let colorSeen = false;
  for (const part of parts) {
    if (styles.has(part)) {
      if (styleSeen) return null;
      styleSeen = true;
    } else if (safeColor.test(part)) {
      if (colorSeen) return null;
      colorSeen = true;
    } else if (safeBorderWidthKeyword.test(part) || (boundedLength(part) !== null && !part.endsWith("%"))) {
      if (widthSeen) return null;
      widthSeen = true;
    } else return null;
  }
  return trimmed;
}

function boxShadow(value: string): string | null {
  const trimmed = value.trim().toLowerCase();
  if (trimmed === "none") return trimmed;
  const parts = trimmed.split(/\s+/);
  if (parts[0] === "inset") parts.shift();
  if (parts.length < 4 || parts.length > 5 || !safeColor.test(parts.at(-1) ?? "")) return null;
  const lengths = parts.slice(0, -1);
  if (!lengths.every((part) => /^-?\d+(?:\.\d+)?px$/.test(part) && Math.abs(Number.parseFloat(part)) <= EMAIL_CSS_LIMITS.maxAbsolutePx)) return null;
  return trimmed;
}

type Validator = (value: string) => string | null;
const regex = (pattern: RegExp): Validator => (value) => pattern.test(value.trim().toLowerCase()) ? value.trim().toLowerCase() : null;
const keyword = (values: string[]) => regex(namedValue(values));
const layoutLength = (options: { allowNegative?: boolean; allowPercentage?: boolean } = {}): Validator => (value) => boundedLength(value, options);
const autoOr = (validator: Validator): Validator => (value) => {
  const trimmed = value.trim().toLowerCase();
  return trimmed === "auto" ? trimmed : validator(value);
};

const validators: Record<string, Validator> = {
  "text-align": keyword(["left", "right", "center", "justify", "start", "end"]),
  "font-weight": regex(/^(normal|bold|[1-9]00)$/),
  "font-style": keyword(["normal", "italic", "oblique"]),
  "font-family": regex(safeFontFamily),
  "text-decoration": regex(/^(none|underline|line-through)( (underline|line-through))?$/),
  "text-transform": keyword(["none", "uppercase", "lowercase", "capitalize"]),
  "vertical-align": keyword(["baseline", "top", "middle", "bottom", "sub", "super", "text-top", "text-bottom"]),
  "white-space": keyword(["normal", "nowrap", "pre", "pre-wrap", "pre-line"]),
  "word-break": keyword(["normal", "break-all", "keep-all", "break-word"]),
  "border-collapse": keyword(["collapse", "separate"]),
  "border-spacing": (value) => {
    const parts = value.trim().toLowerCase().split(/\s+/);
    return parts.length <= 2 && parts.every((part) => boundedLength(part) !== null) ? parts.join(" ") : null;
  },
  background: (value) => safeColor.test(value.trim()) || /^(none|transparent)$/.test(value.trim().toLowerCase()) ? value.trim().toLowerCase() : null,
  color: regex(safeColor),
  "background-color": regex(safeColor),
  opacity,
  "font-size": fontSize,
  "line-height": lineHeight,
  width: autoOr(layoutLength({ allowPercentage: true })),
  height: autoOr(layoutLength({ allowPercentage: true })),
  "min-width": autoOr(layoutLength({ allowPercentage: true })),
  "max-width": (value) => /^(none|auto)$/.test(value.trim().toLowerCase()) ? value.trim().toLowerCase() : boundedLength(value, { allowPercentage: true }),
  "min-height": autoOr(layoutLength({ allowPercentage: true })),
  "max-height": (value) => /^(none|auto)$/.test(value.trim().toLowerCase()) ? value.trim().toLowerCase() : boundedLength(value, { allowPercentage: true }),
  "margin": (value) => value.trim().toLowerCase().split(/\s+/).every((part) => /^(auto|0)$/.test(part) || boundedLength(part, { allowNegative: true, allowPercentage: true }) !== null) ? value.trim().toLowerCase() : null,
  padding: (value) => value.trim().toLowerCase().split(/\s+/).length <= 4 && value.trim().toLowerCase().split(/\s+/).every((part) => boundedLength(part, { allowPercentage: true }) !== null) ? value.trim().toLowerCase() : null,
  "margin-top": layoutLength({ allowNegative: true, allowPercentage: true }),
  "margin-right": layoutLength({ allowNegative: true, allowPercentage: true }),
  "margin-bottom": layoutLength({ allowNegative: true, allowPercentage: true }),
  "margin-left": layoutLength({ allowNegative: true, allowPercentage: true }),
  "padding-top": layoutLength({ allowPercentage: true }),
  "padding-right": layoutLength({ allowPercentage: true }),
  "padding-bottom": layoutLength({ allowPercentage: true }),
  "padding-left": layoutLength({ allowPercentage: true }),
  border,
  "border-top": border,
  "border-right": border,
  "border-bottom": border,
  "border-left": border,
  "border-color": (value) => value.trim().toLowerCase().split(/\s+/).length <= 4 && value.trim().toLowerCase().split(/\s+/).every((part) => safeColor.test(part)) ? value.trim().toLowerCase() : null,
  "border-style": keyword(["none", "solid", "dashed", "dotted", "double", "groove", "ridge", "inset", "outset"]),
  "border-width": (value) => value.trim().toLowerCase().split(/\s+/).length <= 4 && value.trim().toLowerCase().split(/\s+/).every((part) => safeBorderWidthKeyword.test(part) || boundedLength(part) !== null) ? value.trim().toLowerCase() : null,
  "border-radius": (value) => value.trim().toLowerCase().split(/\s+/).every((part) => boundedLength(part, { allowPercentage: true }) !== null) ? value.trim().toLowerCase() : null,
  "box-shadow": boxShadow,
  "box-sizing": keyword(["content-box", "border-box"]),
  display: keyword(["none", "block", "inline", "inline-block", "table", "table-cell", "table-row", "inline-table", "flex", "inline-flex"]),
  visibility: keyword(["visible", "hidden", "collapse"]),
  float: keyword(["none", "left", "right"]),
  clear: keyword(["none", "left", "right", "both"]),
  overflow: keyword(["visible", "hidden", "scroll", "auto"]),
  "overflow-x": keyword(["visible", "hidden", "scroll", "auto"]),
  "overflow-y": keyword(["visible", "hidden", "scroll", "auto"]),
  "table-layout": keyword(["auto", "fixed"]),
  "flex-direction": keyword(["row", "row-reverse", "column", "column-reverse"]),
  "flex-wrap": keyword(["nowrap", "wrap", "wrap-reverse"]),
  flex: (value) => {
    const trimmed = value.trim().toLowerCase();
    if (/^(none|auto|initial|inherit)$/.test(trimmed)) return trimmed;
    const parts = trimmed.split(/\s+/);
    if (parts.length < 1 || parts.length > 3 || unitlessFactor(parts[0]) === null) return null;
    if (parts[1] !== undefined && unitlessFactor(parts[1]) === null) return null;
    if (parts[2] !== undefined && !/^(?:auto|content|0|\d+(?:\.\d+)?(?:px|em|rem|%))$/.test(parts[2])) return null;
    if (parts[2] !== undefined && !/^(?:auto|content|0)$/.test(parts[2]) && boundedLength(parts[2], { allowPercentage: true }) === null) return null;
    return trimmed;
  },
  "flex-grow": unitlessFactor,
  "flex-shrink": unitlessFactor,
  "flex-basis": (value) => /^(auto|content)$/.test(value.trim().toLowerCase()) ? value.trim().toLowerCase() : boundedLength(value, { allowPercentage: true }),
  "align-items": keyword(["stretch", "flex-start", "flex-end", "center", "baseline", "start", "end"]),
  "align-self": keyword(["auto", "stretch", "flex-start", "flex-end", "center", "baseline", "start", "end"]),
  "align-content": keyword(["normal", "stretch", "flex-start", "flex-end", "center", "space-between", "space-around", "space-evenly", "start", "end"]),
  "justify-content": keyword(["normal", "flex-start", "flex-end", "center", "space-between", "space-around", "space-evenly", "start", "end"]),
  gap: layoutLength({ allowPercentage: true }),
  "row-gap": layoutLength({ allowPercentage: true }),
  "column-gap": layoutLength({ allowPercentage: true }),
  "mix-blend-mode": keyword(["normal", "multiply", "screen", "overlay", "darken", "lighten", "color-dodge", "color-burn", "hard-light", "soft-light", "difference", "exclusion", "hue", "saturation", "color", "luminosity"]),
  "letter-spacing": (value) => value.trim().toLowerCase() === "normal" ? "normal" : layoutLength({ allowNegative: true })(value),
  position: keyword(["static", "relative", "absolute"]),
  top: layoutLength({ allowNegative: true, allowPercentage: true }),
  right: layoutLength({ allowNegative: true, allowPercentage: true }),
  bottom: layoutLength({ allowNegative: true, allowPercentage: true }),
  left: layoutLength({ allowNegative: true, allowPercentage: true }),
  font: (value) => {
    const match = value.trim().toLowerCase().match(/^(?:(?:normal|italic|oblique)\s+)?(?:(?:normal|small-caps)\s+)?(?:(?:normal|bold|[1-9]00)\s+)?(\d+(?:\.\d+)?(?:px|em|rem|%))(?:\/(normal|\d+(?:\.\d+)?(?:px|em|rem|%)))?\s+(.+)$/i);
    if (!match || fontSize(match[1]) === null || (match[2] && lineHeight(match[2]) === null) || !safeFontFamily.test(match[3])) return null;
    return value.trim().toLowerCase();
  },
};

export function sanitizeCssDeclaration(property: string, value: string): string | null {
  const validator = validators[property.trim().toLowerCase()];
  if (!validator || /url\s*\(/i.test(value)) return null;
  return validator(value);
}

/**
 * Height to give the message iframe for a document of `contentHeight` CSS
 * pixels. Past the cap the frame stops growing and its document scrolls
 * internally, so tall content stays reachable without an unbounded layout.
 */
export function emailFrameHeight(contentHeight: number): number {
  if (!Number.isFinite(contentHeight) || contentHeight <= 0) return 0;
  return Math.min(contentHeight, EMAIL_CSS_LIMITS.maxFrameHeightPx);
}

export function sanitizeHtmlDimension(value: string, allowPercent: boolean): string | null {
  const trimmed = value.trim();
  if (!/^\d+(?:\.\d+)?%?$/.test(trimmed)) return null;
  const amount = Number(trimmed.replace(/%$/, ""));
  if (!Number.isFinite(amount)) return null;
  if (trimmed.endsWith("%")) return allowPercent && amount <= EMAIL_CSS_LIMITS.maxPercentage ? trimmed : null;
  return amount <= EMAIL_CSS_LIMITS.maxAbsolutePx ? trimmed : null;
}

export function isAllowedCssProperty(property: string): boolean {
  return Object.hasOwn(validators, property.trim().toLowerCase());
}

export const safeStyles = validators;
export const safeStyleProperties = Object.keys(validators);
export const backgroundImageUrl = /^url\((?:"([^"]*)"|'([^']*)'|([^'")]*))\)$/i;
export const safeImageSrc = /^(https?:|cid:|data:image\/)/i;
export const blockedSrcAttr = "data-blocked-src";
