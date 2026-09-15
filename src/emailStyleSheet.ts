import postcss from "postcss";
import selectorParser from "postcss-selector-parser";
import { sanitizeCssDeclaration } from "./emailRenderingPolicy";

const MEDIA_FEATURE = /^(?:\(\s*(?:min-width|max-width)\s*:\s*(\d+(?:\.\d+)?px)\s*\)|\(\s*(?:orientation)\s*:\s*(?:portrait|landscape)\s*\)|\(\s*prefers-color-scheme\s*:\s*(?:dark|light)\s*\))$/i;

function isSafeMediaQuery(query: string): boolean {
  const trimmed = query.trim();
  if (!trimmed) return false;
  return trimmed.split(/\s+and\s+/i).every((part) => {
    if (/^(?:only\s+)?(?:screen|all)$/i.test(part.trim())) return true;
    const feature = part.trim().match(/^\(\s*(min-width|max-width)\s*:\s*([^)]*)\)$/i);
    if (feature) return sanitizeCssDeclaration("width", feature[2]) !== null && MEDIA_FEATURE.test(part.trim());
    return MEDIA_FEATURE.test(part.trim());
  });
}

function scopedSelectorList(selectorText: string): string | null {
  try {
    const parser = selectorParser();
    const ast = parser.astSync(selectorText);
    const selectors: string[] = [];
    ast.each((selector) => {
      let hasRoot = false;
      selector.walkTags((tag) => {
        if (/^(?:html|body)$/i.test(tag.value)) {
          tag.replaceWith(selectorParser.attribute({ attribute: "data-email-root", value: undefined, raws: {} }));
          hasRoot = true;
        }
      });
      selector.walkPseudos((pseudo) => {
        if (pseudo.value.toLowerCase() === ":root") {
          pseudo.replaceWith(selectorParser.attribute({ attribute: "data-email-root", value: undefined, raws: {} }));
          hasRoot = true;
        }
      });
      selector.walkAttributes((attribute) => {
        if (attribute.attribute === "data-email-root") hasRoot = true;
      });
      if (!selector.toString().trim()) return;
      if (!hasRoot) {
        selector.prepend(selectorParser.combinator({ value: " " }));
        selector.prepend(selectorParser.attribute({ attribute: "data-email-root", value: undefined, raws: {} }));
      }
      selectors.push(selector.toString().trim().replace(/^(\[data-email-root\])\s+/, "$1 "));
    });
    return selectors.length > 0 ? selectors.join(", ") : null;
  } catch {
    return null;
  }
}

/**
 * Sanitizes sender CSS with the same typed declaration policy used for
 * inline styles. PostCSS owns stylesheet structure; selector-parser owns
 * selector-list parsing, root rewriting, and malformed-selector rejection.
 * Only bounded responsive media queries survive. Unsupported at-rules and
 * active/resource declarations are removed.
 */
export function sanitizeStyleSheet(css: string, theme?: "light" | "dark"): string {
  let root;
  try {
    root = postcss.parse(css);
  } catch {
    return "";
  }

  root.walkAtRules((atRule) => {
    if (atRule.name.toLowerCase() !== "media" || !atRule.params.split(",").every(isSafeMediaQuery)) {
      atRule.remove();
      return;
    }
    if (!theme) return;

    const branches = atRule.params.split(",").map((branch) => branch.trim());
    const preferredThemes = branches.map((branch) => branch.match(/prefers-color-scheme\s*:\s*(dark|light)/i)?.[1]?.toLowerCase());
    const specifiedThemes = preferredThemes.filter((value): value is string => value !== undefined);
    if (specifiedThemes.length > 0) {
      // A comma list mixing themed and unthemed branches cannot be faithfully
      // mapped to the selected Dispatch theme without changing its meaning.
      if (specifiedThemes.length !== branches.length) {
        atRule.remove();
        return;
      }
      // Mixed light/dark comma branches are ambiguous after mapping to the
      // reader-selected theme; drop them instead of falling back to the OS.
      if (specifiedThemes.some((value) => value !== theme) || specifiedThemes.some((value) => value !== specifiedThemes[0])) {
        atRule.remove();
        return;
      }
      atRule.params = branches
        .map((branch) => branch.replace(/\s+and\s*\(\s*prefers-color-scheme\s*:\s*(?:dark|light)\s*\)/i, "").replace(/^\(\s*prefers-color-scheme\s*:\s*(?:dark|light)\s*\)\s*and\s*/i, "").replace(/^\(\s*prefers-color-scheme\s*:\s*(?:dark|light)\s*\)$/i, "").trim())
        .filter(Boolean)
        .join(", ") || "all";
      atRule.walkRules((rule) => {
        const scoped = scopedSelectorList(rule.selector);
        if (scoped) {
          rule.selector = scoped.replaceAll(
            "[data-email-root]",
            `[data-email-root][data-theme="${theme}"]`,
          );
        }
      });
    }
  });

  root.walkRules((rule) => {
    const selector = scopedSelectorList(rule.selector);
    if (!selector) {
      rule.remove();
      return;
    }
    rule.selector = selector;

    rule.walkDecls((decl) => {
      const safeValue = sanitizeCssDeclaration(decl.prop, decl.value);
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
