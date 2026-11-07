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
 * Removes the CDO (`<!--`) and CDC (`-->`) tokens that sit between rules.
 * CSS Syntax ignores them at a stylesheet's top level, so senders wrap
 * `<style>` content in them to hide it from pre-CSS browsers; PostCSS
 * instead rejects `-->` as an unknown word and the whole sheet would be
 * lost. Markers inside strings, comments, or blocks are left alone, and
 * each removed marker becomes a space so its neighbours cannot fuse into
 * a new token.
 */
function stripTopLevelHtmlCommentMarkers(css: string): string {
  let out = "";
  let depth = 0;
  let quote: string | null = null;
  let i = 0;
  while (i < css.length) {
    const ch = css[i];
    if (quote) {
      if (ch === "\\") {
        out += css.slice(i, i + 2);
        i += 2;
        continue;
      }
      if (ch === quote || ch === "\n") quote = null;
      out += ch;
      i += 1;
      continue;
    }
    if (ch === "/" && css[i + 1] === "*") {
      const end = css.indexOf("*/", i + 2);
      const stop = end === -1 ? css.length : end + 2;
      out += css.slice(i, stop);
      i = stop;
      continue;
    }
    if (ch === "\\") {
      out += css.slice(i, i + 2);
      i += 2;
      continue;
    }
    if (ch === '"' || ch === "'") quote = ch;
    else if (ch === "{") depth += 1;
    else if (ch === "}" && depth > 0) depth -= 1;
    else if (depth === 0 && css.startsWith("<!--", i)) {
      out += " ";
      i += 4;
      continue;
    } else if (depth === 0 && css.startsWith("-->", i)) {
      out += " ";
      i += 3;
      continue;
    }
    out += ch;
    i += 1;
  }
  return out;
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
    root = postcss.parse(stripTopLevelHtmlCommentMarkers(css));
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
      // mapped to the selected ThreeStrands theme without changing its meaning.
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
