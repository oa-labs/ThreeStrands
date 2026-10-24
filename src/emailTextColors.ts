import {
  belowMinimumContrast,
  blockedSrcAttr,
  isDeliberatelyHiddenText,
  parseEmailTextColors,
  type EmailTextColors,
} from "./emailRenderingPolicy";

export type Rgba = { r: number; g: number; b: number; a: number };

const WHITE: Rgba = { r: 255, g: 255, b: 255, a: 1 };
const BLACK: Rgba = { r: 0, g: 0, b: 0, a: 1 };

/** Parses the rgb()/rgba() form getComputedStyle reports for colors. */
export function parseComputedColor(value: string): Rgba | null {
  const match = value.trim().match(/^rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)(?:\s*[,/]\s*([\d.]+)(%?))?\s*\)$/i);
  if (!match) return null;
  const [r, g, b] = [match[1], match[2], match[3]].map(Number);
  const alpha = match[4] === undefined ? 1 : Number(match[4]) / (match[5] ? 100 : 1);
  if (![r, g, b, alpha].every(Number.isFinite)) return null;
  return { r, g, b, a: Math.min(1, Math.max(0, alpha)) };
}

function channelLuminance(channel: number): number {
  const value = channel / 255;
  return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
}

function luminance({ r, g, b }: Rgba): number {
  return 0.2126 * channelLuminance(r) + 0.7152 * channelLuminance(g) + 0.0722 * channelLuminance(b);
}

/** WCAG contrast ratio between two opaque colors. */
export function contrastRatio(first: Rgba, second: Rgba): number {
  const [light, dark] = [luminance(first), luminance(second)].sort((a, b) => b - a);
  return (light + 0.05) / (dark + 0.05);
}

/** Paints `top` over an opaque `bottom`. */
export function composite(top: Rgba, bottom: Rgba): Rgba {
  const blend = (front: number, back: number) => front * top.a + back * (1 - top.a);
  return { r: blend(top.r, bottom.r), g: blend(top.g, bottom.g), b: blend(top.b, bottom.b), a: 1 };
}

function toHsl({ r, g, b }: Rgba): [number, number, number] {
  const [red, green, blue] = [r / 255, g / 255, b / 255];
  const max = Math.max(red, green, blue);
  const min = Math.min(red, green, blue);
  const lightness = (max + min) / 2;
  if (max === min) return [0, 0, lightness];
  const delta = max - min;
  const saturation = delta / (1 - Math.abs(2 * lightness - 1));
  const hue = max === red ? ((green - blue) / delta) % 6
    : max === green ? (blue - red) / delta + 2
      : (red - green) / delta + 4;
  return [hue * 60, saturation, lightness];
}

function fromHsl(hue: number, saturation: number, lightness: number): Rgba {
  const chroma = (1 - Math.abs(2 * lightness - 1)) * saturation;
  const x = chroma * (1 - Math.abs(((hue / 60) % 2) - 1));
  const m = lightness - chroma / 2;
  const sector = Math.floor((((hue % 360) + 360) % 360) / 60);
  const [red, green, blue] = [
    [chroma, x, 0], [x, chroma, 0], [0, chroma, x],
    [0, x, chroma], [x, 0, chroma], [chroma, 0, x],
  ][sector];
  const channel = (value: number) => Math.round(Math.min(1, Math.max(0, value + m)) * 255);
  return { r: channel(red), g: channel(green), b: channel(blue), a: 1 };
}

/**
 * The closest color to `text` (same hue and saturation, lightness moved
 * toward whichever of white or black contrasts more with `background`) that
 * meets the minimum contrast ratio. Both colors must be opaque.
 */
export function readableTextColor(text: Rgba, background: Rgba): Rgba {
  const lighten = contrastRatio(WHITE, background) >= contrastRatio(BLACK, background);
  const [hue, saturation, lightness] = toHsl(text);
  const target = lighten ? 1 : 0;
  const at = (step: number) => fromHsl(hue, saturation, lightness + (target - lightness) * step);
  // The extreme is always readable: white or black reaches at least 4.58:1
  // against any background. Search for the smallest change that also is.
  let low = 0;
  let high = 1;
  for (let iteration = 0; iteration < 16; iteration += 1) {
    const middle = (low + high) / 2;
    if (belowMinimumContrast(contrastRatio(at(middle), background))) low = middle;
    else high = middle;
  }
  return at(high);
}

function cssColor({ r, g, b }: Rgba): string {
  return `rgb(${Math.round(r)}, ${Math.round(g)}, ${Math.round(b)})`;
}

/** The opaque background behind an element, or null when an image may be painted under its text. */
type Backdrop = { color: Rgba; senderPainted: boolean } | null;

/**
 * Runs only in the sanitized, CSP-scoped message document. These trusted
 * overrides write opaque rgb() colors and `inherit`/theme variables only:
 * no resources, scripts, navigation, or interaction capabilities are added.
 * Text painted over a background image or gradient keeps sender colors,
 * since its real contrast cannot be measured.
 */
export function createEmailTextColorController(doc: Document): (value: EmailTextColors) => void {
  const originals = new Map<HTMLElement, { property: string; value: string; priority: string }[]>();
  const remember = (element: HTMLElement, property: string) => {
    const declarations = originals.get(element) ?? [];
    if (!declarations.some((declaration) => declaration.property === property)) {
      declarations.push({
        property,
        value: element.style.getPropertyValue(property),
        priority: element.style.getPropertyPriority(property),
      });
    }
    originals.set(element, declarations);
  };
  return (value) => {
    for (const [element, declarations] of originals) {
      for (const declaration of declarations) {
        if (declaration.value) element.style.setProperty(declaration.property, declaration.value, declaration.priority);
        else element.style.removeProperty(declaration.property);
      }
    }
    originals.clear();
    const mode = parseEmailTextColors(value);
    const view = doc.defaultView;
    const root = doc.querySelector<HTMLElement>("[data-email-root]");
    if (mode === "sender" || !view || !root) return;

    // The frame is transparent over the message card; --canvas names its
    // color. Without it no contrast can be measured, so sender colors stay.
    const canvas = parseComputedColor(view.getComputedStyle(doc.documentElement).getPropertyValue("--canvas"));
    if (!canvas) return;
    // Read the entire baseline before writing, so one override cannot
    // change what a later element measures.
    const backdrops = new Map<Element, Backdrop>();
    const backdropOf = (element: Element | null): Backdrop => {
      if (!element) return { color: canvas, senderPainted: false };
      const cached = backdrops.get(element);
      if (cached !== undefined) return cached;
      const computed = view.getComputedStyle(element);
      let backdrop: Backdrop;
      if (element.hasAttribute(blockedSrcAttr) || (computed.backgroundImage && computed.backgroundImage !== "none")) {
        backdrop = null;
      } else {
        const parent = backdropOf(element.parentElement);
        const own = parseComputedColor(computed.backgroundColor);
        backdrop = !parent || !own || own.a === 0
          ? parent
          : { color: composite(own, parent.color), senderPainted: parent.senderPainted || (element !== root && root.contains(element)) };
      }
      backdrops.set(element, backdrop);
      return backdrop;
    };

    const elements = Array.from(root.querySelectorAll<HTMLElement>("*"));
    // Text in the reader's own color was not colored by the sender.
    const readerColor = view.getComputedStyle(root).color;
    if (mode === "app") {
      const writes = elements
        .filter((element) => backdropOf(element) !== null)
        .map((element) => ({ element, link: element.matches("a[href]") }));
      for (const { element, link } of writes) {
        remember(element, "color");
        remember(element, "background-color");
        element.style.setProperty("color", link ? "var(--link)" : "inherit", "important");
        element.style.setProperty("background-color", "transparent", "important");
      }
      return;
    }

    const writes = elements
      .filter((element) => Array.from(element.childNodes)
        .some((node) => node.nodeType === Node.TEXT_NODE && node.textContent?.trim()))
      .flatMap((element) => {
        const backdrop = backdropOf(element);
        const computedColor = view.getComputedStyle(element).color;
        const color = parseComputedColor(computedColor);
        if (!backdrop || !color || color.a === 0) return [];
        const text = composite(color, backdrop.color);
        const ratio = contrastRatio(text, backdrop.color);
        if (!belowMinimumContrast(ratio)) return [];
        // Only a sender who chose both colors can have hidden text on purpose.
        if (backdrop.senderPainted && computedColor !== readerColor && isDeliberatelyHiddenText(ratio)) return [];
        return [{ element, color: cssColor(readableTextColor(text, backdrop.color)) }];
      });
    for (const { element, color } of writes) {
      remember(element, "color");
      element.style.setProperty("color", color, "important");
    }
  };
}
