import { EMAIL_MINIMUM_FONT_SIZE, parseEmailMinimumFontSize } from "./emailRenderingPolicy";

/**
 * Runs only in the sanitized, CSP-scoped message document. These trusted
 * typography overrides contain numeric lengths only: no resources, scripts,
 * navigation, or interaction capabilities are added to sender CSS.
 */
export function createEmailFontSizeController(doc: Document): (value: number) => void {
  const originals = new Map<HTMLElement, { property: string; value: string; priority: string }[]>();
  return (value) => {
    for (const [element, declarations] of originals) {
      for (const declaration of declarations) {
        if (declaration.value) element.style.setProperty(declaration.property, declaration.value, declaration.priority);
        else element.style.removeProperty(declaration.property);
      }
    }
    originals.clear();
    const minimum = parseEmailMinimumFontSize(value);
    const view = doc.defaultView;
    if (!minimum || !view || !doc.body) return;

    // Read the entire baseline before writing. Relative sizes must resolve
    // against sender typography, not an ancestor we have already enlarged.
    const measurements = Array.from(doc.body.querySelectorAll<HTMLElement>("*"))
      .filter((element) => Array.from(element.childNodes)
        .some((node) => node.nodeType === Node.TEXT_NODE && node.textContent?.trim()))
      .map((element) => {
        const computed = view.getComputedStyle(element);
        return { element, size: Number.parseFloat(computed.fontSize), lineHeight: Number.parseFloat(computed.lineHeight) };
      })
      // Zero-sized text and whitespace-only spacers are layout structure.
      .filter(({ size }) => Number.isFinite(size) && size > 0);
    if (!measurements.some(({ size }) => size < minimum)) return;

    for (const { element, size, lineHeight } of measurements) {
      // Restore typography only: the image proxy can resolve background
      // images after load, and those updates must survive font changes.
      originals.set(element, ["font-size", "line-height"].map((property) => ({
        property,
        value: element.style.getPropertyValue(property),
        priority: element.style.getPropertyPriority(property),
      })));
      // Pin larger descendants too, so relative headings do not grow when
      // a text-bearing ancestor is enlarged. Larger text keeps its size.
      element.style.setProperty("font-size", `${Math.max(size, minimum)}px`, "important");
      if (size < minimum && Number.isFinite(lineHeight)) {
        element.style.setProperty("line-height", `${Math.max(
          lineHeight,
          minimum * EMAIL_MINIMUM_FONT_SIZE.minLineHeightRatio,
        )}px`, "important");
      }
    }
  };
}
