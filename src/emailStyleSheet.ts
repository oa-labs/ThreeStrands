import postcss from "postcss";
import { safeStyles } from "./emailSafeStyles";

// The one legitimate reason to want a <style> block at all: ESP templates
// (Customer.io, Klaviyo, HubSpot) commonly ship two variants of a logo/icon
// — one plain, one under a class a media query shows only in dark mode — to
// work around Gmail's automatic color inversion. Nothing else gets in:
// @font-face/@import/@keyframes/@supports and any other media feature are
// all dropped, along with every other at-rule.
const ALLOWED_MEDIA_QUERY = /^\(\s*prefers-color-scheme\s*:\s*(dark|light)\s*\)$/i;

// Rejects any selector that could reach outside the message body content
// itself — html/body/:root (the whole rendered surface) or a bare universal
// selector. Everything else (type/class/id/attribute selectors, pseudo
// classes/elements, combinators) is left alone; the browser's own selector
// engine resolves it exactly like any other CSS, and matching a class
// attribute is only possible against elements/attributes DOMPurify already
// allowed through.
const UNSAFE_SELECTOR_TARGET = /(^|[\s,>+~])(html|body)(?=$|[\s,>+~.:#[])|:root\b|(^|[\s,>+~])\*(?=$|[\s,>+~.:#[])/i;

function isSafeSelector(selector: string): boolean {
  const trimmed = selector.trim();
  return trimmed.length > 0 && !UNSAFE_SELECTOR_TARGET.test(trimmed);
}

// No property in `safeStyles` is expected to carry a url() — background-image
// specifically isn't in that map, on purpose. A <style> rule's selector can
// match zero, one, or many not-yet-known elements, so there's no single DOM
// node to gate behind the image-blocking flow the way inline
// style="background-image:..." and <img src> both are (see SafeMessage.tsx).
// Rather than build a second, weaker gating path for this one spot, any
// declaration whose value contains url(...) is dropped unconditionally.
function sanitizeDeclarationValue(prop: string, rawValue: string): string | null {
  const property = prop.trim().toLowerCase();
  const pattern = safeStyles[property];
  if (!pattern) return null;
  const value = rawValue.trim();
  if (/url\(/i.test(value)) return null;
  const normalized = value.toLowerCase();
  return pattern.test(normalized) ? normalized : null;
}

/**
 * Sanitizes the text content of a `<style>` tag down to the same
 * property/value allowlist SafeMessage.tsx applies to inline `style=""`
 * attributes, plus a small selector allowlist and an at-rule allowlist of
 * exactly `@media (prefers-color-scheme: ...)`. Returns "" if nothing in
 * the sheet survives (including if it fails to parse at all).
 */
export function sanitizeStyleSheet(css: string): string {
  let root;
  try {
    root = postcss.parse(css);
  } catch {
    return "";
  }

  root.walkAtRules((atRule) => {
    if (atRule.name.toLowerCase() !== "media" || !ALLOWED_MEDIA_QUERY.test(atRule.params.trim())) {
      atRule.remove();
    }
  });

  root.walkRules((rule) => {
    const selectors = rule.selector.split(",").map((s) => s.trim()).filter(isSafeSelector);
    if (selectors.length === 0) {
      rule.remove();
      return;
    }
    rule.selector = selectors.join(", ");

    rule.walkDecls((decl) => {
      const safeValue = sanitizeDeclarationValue(decl.prop, decl.value);
      if (safeValue === null) decl.remove();
      else decl.value = safeValue;
    });

    if (rule.nodes.length === 0) rule.remove();
  });

  // A retained @media block whose only rule(s) got fully stripped above is
  // now dead weight.
  root.walkAtRules((atRule) => {
    if (atRule.nodes && atRule.nodes.length === 0) atRule.remove();
  });

  // The result gets embedded as the raw text content of an HTML <style>
  // element (see buildMessageDocument). Raw text elements end at the first
  // literal `<` the parser can read as a tag open — not at anything CSS
  // itself considers meaningful — and quoted attribute-selector values
  // (e.g. `[title="</style><script>..."]`) are free-form strings PostCSS
  // has no reason to reject. Escaping every `<` is the same technique
  // browsers use for entities in this context: raw text is never
  // entity-decoded, so `&lt;` can't be reassembled into a real tag close,
  // regardless of where in the sheet it came from.
  return root.toString().trim().replace(/</g, "&lt;");
}
