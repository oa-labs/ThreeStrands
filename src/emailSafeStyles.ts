// Backwards-compatible module surface for callers that imported the old
// email-safe style helpers. The implementation now lives in the single
// renderer policy so inline CSS, embedded styles, and HTML dimensions cannot
// drift apart again.
export {
  EMAIL_CSS_LIMITS,
  backgroundImageUrl,
  blockedSrcAttr,
  safeColor,
  safeImageSrc,
  safeStyles,
  safeStyleProperties,
  sanitizeCssDeclaration,
  sanitizeHtmlDimension,
} from "./emailRenderingPolicy";
