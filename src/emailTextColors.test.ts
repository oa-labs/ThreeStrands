import { afterEach, describe, expect, it } from "vitest";
import { EMAIL_TEXT_COLORS } from "./emailRenderingPolicy";
import {
  composite,
  contrastRatio,
  createEmailTextColorController,
  parseComputedColor,
  readableTextColor,
  type Rgba,
} from "./emailTextColors";

const DARK_CANVAS = "rgb(29, 29, 33)";
const LIGHT_CANVAS = "rgb(255, 255, 255)";

/** Mirrors the message document: theme tokens on :root, sender markup under the email root. */
function messageDocument(canvas: string, html: string): Document {
  const text = canvas === DARK_CANVAS ? "#d2d2d7" : "#353541";
  document.head.innerHTML = `<style>:root { --canvas: ${canvas}; --link: #aaa6ff; } body { color: ${text}; }</style>`;
  document.body.innerHTML = `<div class="email-root" data-email-root>${html}</div>`;
  return document;
}

function rgb(value: string): Rgba {
  return parseComputedColor(value)!;
}

function contrastOn(element: HTMLElement, background: string): number {
  return contrastRatio(rgb(getComputedStyle(element).color), rgb(background));
}

function hue({ r, g, b }: Rgba): "red" | "green" | "blue" {
  return r >= g && r >= b ? "red" : g >= b ? "green" : "blue";
}

afterEach(() => {
  document.head.innerHTML = "";
  document.body.innerHTML = "";
});

describe("email text color math", () => {
  it("parses the color forms getComputedStyle reports", () => {
    expect(parseComputedColor("rgb(0, 0, 238)")).toEqual({ r: 0, g: 0, b: 238, a: 1 });
    expect(parseComputedColor("rgb(29,29,33)")).toEqual({ r: 29, g: 29, b: 33, a: 1 });
    expect(parseComputedColor("rgba(255, 255, 255, 0.5)")).toEqual({ r: 255, g: 255, b: 255, a: 0.5 });
    expect(parseComputedColor("rgb(1 2 3 / 40%)")).toEqual({ r: 1, g: 2, b: 3, a: 0.4 });
    for (const value of ["", "transparent", "#fff", "url(https://tracker.invalid)", "rgb(1, 2)"]) {
      expect(parseComputedColor(value)).toBeNull();
    }
  });

  it("measures WCAG contrast and composites translucent layers", () => {
    expect(contrastRatio(rgb("rgb(255, 255, 255)"), rgb("rgb(0, 0, 0)"))).toBeCloseTo(21);
    expect(contrastRatio(rgb("rgb(119, 119, 119)"), rgb("rgb(119, 119, 119)"))).toBe(1);
    expect(composite(rgb("rgba(255, 255, 255, 0.5)"), rgb("rgb(0, 0, 0)"))).toEqual({ r: 127.5, g: 127.5, b: 127.5, a: 1 });
  });

  it.each([
    ["saturated blue on the dark canvas", "rgb(0, 0, 238)", DARK_CANVAS],
    ["mid gray on the dark canvas", "rgb(102, 102, 102)", DARK_CANVAS],
    ["yellow on the light canvas", "rgb(255, 230, 0)", LIGHT_CANVAS],
    ["gray on a mid-gray sender panel", "rgb(150, 150, 150)", "rgb(128, 128, 128)"],
  ])("lifts %s to the minimum ratio with the smallest lightness change", (_name, text, background) => {
    const readable = readableTextColor(rgb(text), rgb(background));
    const ratio = contrastRatio(readable, rgb(background));
    expect(ratio).toBeGreaterThanOrEqual(EMAIL_TEXT_COLORS.minContrastRatio);
    // The search stops at the threshold rather than jumping to white or black.
    expect(ratio).toBeLessThan(EMAIL_TEXT_COLORS.minContrastRatio + 0.25);
    if (rgb(text).r !== rgb(text).b) expect(hue(readable)).toBe(hue(rgb(text)));
  });
});

describe("email text color controller", () => {
  it("lifts only failing sender text on the dark canvas and keeps its hue", () => {
    const doc = messageDocument(DARK_CANVAS, [
      '<p id="body" style="color:#0000ee">Only examples of employees</p>',
      '<p id="signature" style="color:#666666">Vice President of IT</p>',
      '<p id="readable" style="color:#e0e0e0">Already readable</p>',
      '<p id="default">Uses the reader color</p>',
    ].join(""));
    const update = createEmailTextColorController(doc);
    update("contrast");
    const body = doc.getElementById("body")!;
    const signature = doc.getElementById("signature")!;
    expect(contrastOn(body, DARK_CANVAS)).toBeGreaterThanOrEqual(EMAIL_TEXT_COLORS.minContrastRatio);
    expect(hue(rgb(body.style.color))).toBe("blue");
    expect(body.style.getPropertyPriority("color")).toBe("important");
    expect(contrastOn(signature, DARK_CANVAS)).toBeGreaterThanOrEqual(EMAIL_TEXT_COLORS.minContrastRatio);
    expect(doc.getElementById("readable")!.getAttribute("style")).toBe("color:#e0e0e0");
    expect(doc.getElementById("default")!.hasAttribute("style")).toBe(false);
  });

  it("measures against sender-painted backgrounds in div and table layouts", () => {
    const doc = messageDocument(DARK_CANVAS, [
      '<div style="background-color:#ffffff"><p id="card" style="color:#0000ee">Blue on a white card</p></div>',
      '<table><tr><td style="background-color:#ffffff"><span id="cell" style="color:#333333">Dark on a white cell</span></td></tr></table>',
      '<table><tr><td style="background-color:#f4f4f4"><span id="faint" style="color:#bbbbbb">Faint on a light cell</span></td></tr></table>',
    ].join(""));
    createEmailTextColorController(doc)("contrast");
    // Readable against the sender's own background, even though the canvas is dark.
    expect(doc.getElementById("card")!.style.color).toBe("rgb(0, 0, 238)");
    expect(doc.getElementById("cell")!.style.color).toBe("rgb(51, 51, 51)");
    // Darkened toward the readable side of the sender background, not the canvas.
    const faint = doc.getElementById("faint")!;
    expect(contrastOn(faint, "rgb(244, 244, 244)")).toBeGreaterThanOrEqual(EMAIL_TEXT_COLORS.minContrastRatio);
    expect(rgb(faint.style.color).r).toBeLessThan(0xbb);
  });

  it("darkens failing text on the light canvas", () => {
    const doc = messageDocument(LIGHT_CANVAS, '<p id="pale" style="color:#ffe600">Pale yellow</p><p id="navy" style="color:#000080">Navy</p>');
    createEmailTextColorController(doc)("contrast");
    expect(contrastOn(doc.getElementById("pale")!, LIGHT_CANVAS)).toBeGreaterThanOrEqual(EMAIL_TEXT_COLORS.minContrastRatio);
    expect(doc.getElementById("navy")!.style.color).toBe("rgb(0, 0, 128)");
  });

  it("composites translucent sender backgrounds and text before measuring", () => {
    const doc = messageDocument(DARK_CANVAS, [
      '<div style="background-color:rgba(255, 255, 255, 0.9)"><p id="dark" style="color:#222222">Dark on a nearly white wash</p></div>',
      '<p id="ghost" style="color:rgba(255, 255, 255, 0.3)">Faded white</p>',
    ].join(""));
    createEmailTextColorController(doc)("contrast");
    expect(doc.getElementById("dark")!.style.color).toBe("rgb(34, 34, 34)");
    const ghost = doc.getElementById("ghost")!;
    expect(contrastOn(ghost, DARK_CANVAS)).toBeGreaterThanOrEqual(EMAIL_TEXT_COLORS.minContrastRatio);
  });

  it("keeps text the sender hid against its own background, but lifts black text left on the canvas", () => {
    const doc = messageDocument(DARK_CANVAS, [
      '<div style="background-color:#ffffff"><span id="preheader" style="color:#ffffff">Preview filler</span></div>',
      '<p id="clear" style="color:transparent">Invisible spacer</p>',
      '<p id="black" style="color:#000000">Written for a white page</p>',
    ].join(""));
    createEmailTextColorController(doc)("contrast");
    expect(doc.getElementById("preheader")!.style.color).toBe("rgb(255, 255, 255)");
    expect(doc.getElementById("clear")!.style.color).toBe("transparent");
    expect(contrastOn(doc.getElementById("black")!, DARK_CANVAS)).toBeGreaterThanOrEqual(EMAIL_TEXT_COLORS.minContrastRatio);
  });

  it("darkens reader-colored text on a light sender panel, since the sender did not choose it", () => {
    const doc = messageDocument(DARK_CANVAS, '<table><tr><td style="background-color:#dcdcdc"><p id="copy">Three highlights</p></td></tr></table>');
    createEmailTextColorController(doc)("contrast");
    expect(contrastOn(doc.getElementById("copy")!, "rgb(220, 220, 220)")).toBeGreaterThanOrEqual(EMAIL_TEXT_COLORS.minContrastRatio);
  });

  it("keeps sender colors over background images and gradients, whose contrast is unknown", () => {
    const doc = messageDocument(DARK_CANVAS, [
      '<div style="background-image:linear-gradient(#000, #fff)"><p id="gradient" style="color:#0000ee">Over a gradient</p></div>',
      '<div data-blocked-src="https://example.com/hero.png"><p id="hero" style="color:#0000ee">Over a blocked image</p></div>',
    ].join(""));
    for (const mode of ["contrast", "app"] as const) {
      createEmailTextColorController(doc)(mode);
      expect(doc.getElementById("gradient")!.style.color).toBe("rgb(0, 0, 238)");
      expect(doc.getElementById("hero")!.style.color).toBe("rgb(0, 0, 238)");
      expect(doc.querySelector<HTMLElement>("[data-blocked-src]")!.style.backgroundColor).toBe("");
    }
  });

  it("replaces sender text and background colors with the theme in app mode", () => {
    const doc = messageDocument(DARK_CANVAS, [
      '<table><tr><td id="cell" style="background-color:#ffffff"><span id="text" style="color:#333333">Body</span>',
      ' <a id="link" href="https://example.com" style="color:#0000ee"><span id="inner">Link</span></a></td></tr></table>',
    ].join(""));
    createEmailTextColorController(doc)("app");
    expect(doc.getElementById("cell")!.style.backgroundColor).toBe("transparent");
    expect(doc.getElementById("text")!.style.color).toBe("inherit");
    expect(doc.getElementById("link")!.style.color).toBe("var(--link)");
    // Content inside a link inherits the link color.
    expect(doc.getElementById("inner")!.style.color).toBe("inherit");
    expect(doc.getElementById("text")!.style.getPropertyPriority("color")).toBe("important");
  });

  it("restores sender declarations, including their priority, when the mode changes", () => {
    const doc = messageDocument(DARK_CANVAS, '<div id="panel" style="background-color:#fafafa"><p id="text" style="color:#0000ee !important">Blue</p></div><p id="plain" style="color:#555555">Gray</p>');
    const update = createEmailTextColorController(doc);
    const styles = () => Array.from(doc.querySelectorAll<HTMLElement>("[data-email-root] *")).map((element) => element.style.cssText);
    const original = styles();
    update("app");
    update("contrast");
    expect(doc.getElementById("panel")!.style.backgroundColor).toBe("rgb(250, 250, 250)");
    expect(doc.getElementById("plain")!.style.color).not.toBe("rgb(85, 85, 85)");
    update("sender");
    expect(styles()).toEqual(original);
    expect(doc.getElementById("text")!.style.getPropertyPriority("color")).toBe("important");
    // Unknown values fall back to the default mode, never to raw CSS.
    update("color:red;background:url(https://tracker.invalid)" as never);
    expect(doc.getElementById("plain")!.style.color).not.toBe("rgb(85, 85, 85)");
    expect(doc.body.innerHTML).not.toContain("tracker.invalid");
  });

  it("keeps sender colors when the document does not name its canvas", () => {
    const doc = messageDocument(DARK_CANVAS, '<p id="blue" style="color:#0000ee">Blue</p>');
    document.head.innerHTML = "";
    for (const mode of ["contrast", "app"] as const) {
      createEmailTextColorController(doc)(mode);
      expect(doc.getElementById("blue")!.getAttribute("style")).toBe("color:#0000ee");
    }
  });

  it("writes only opaque colors and theme references, never resources", () => {
    const doc = messageDocument(DARK_CANVAS, '<div style="background-color:#ffffff"><p style="color:#eeeeee">Light on white</p></div><p style="color:#111111">Dark on canvas</p><a href="https://example.com" style="color:#000">Link</a>');
    for (const mode of ["contrast", "app"] as const) {
      createEmailTextColorController(doc)(mode);
      for (const element of Array.from(doc.querySelectorAll<HTMLElement>("[data-email-root] *"))) {
        for (const property of ["color", "background-color"]) {
          const value = element.style.getPropertyValue(property);
          if (element.style.getPropertyPriority(property) !== "important") continue;
          expect(value).toMatch(/^(rgb\(\d+, \d+, \d+\)|inherit|transparent|var\(--link\))$/);
        }
      }
      expect(doc.body.innerHTML).not.toMatch(/url\(/i);
    }
  });
});
