import { describe, expect, it } from "vitest";
import { EMAIL_ZOOM, parseEmailZoom, EMAIL_MINIMUM_FONT_SIZE, parseEmailMinimumFontSize, EMAIL_CSS_LIMITS, EMAIL_IMAGE_LIMITS, preservedImageMinWidth, emailFrameHeight, sanitizeCssDeclaration, sanitizeHtmlCellPadding, sanitizeHtmlDimension, sanitizeHtmlFontSize, EMAIL_LEGACY_FONT_SIZE } from "./emailRenderingPolicy";

describe("email rendering numeric policy", () => {
  it("validates email zoom at both bounds without clamping or accepting CSS", () => {
    const { minPercent, maxPercent, defaultPercent } = EMAIL_ZOOM;
    for (const value of [minPercent, minPercent + 1, 125, maxPercent - 1, maxPercent]) {
      expect(parseEmailZoom(value)).toBe(value);
    }
    for (const value of [minPercent - 1, maxPercent + 1, 0, -1, 125.5, NaN, Infinity, undefined, "125", "125%;background:url(https://tracker.invalid)"]) {
      expect(parseEmailZoom(value)).toBe(defaultPercent);
    }
    // Magnification is a reader capability, never sender CSS.
    expect(sanitizeCssDeclaration("zoom", "2")).toBeNull();
  });
  it("validates the reader font floor at both boundaries without silently clamping", () => {
    const { minPx, maxPx } = EMAIL_MINIMUM_FONT_SIZE;
    expect(parseEmailMinimumFontSize(0)).toBe(0);
    expect(parseEmailMinimumFontSize(minPx - 1)).toBe(0);
    expect(parseEmailMinimumFontSize(minPx)).toBe(minPx);
    expect(parseEmailMinimumFontSize(minPx + 1)).toBe(minPx + 1);
    expect(parseEmailMinimumFontSize(maxPx - 1)).toBe(maxPx - 1);
    expect(parseEmailMinimumFontSize(maxPx)).toBe(maxPx);
    expect(parseEmailMinimumFontSize(maxPx + 1)).toBe(0);
    for (const value of [-1, 18.5, NaN, Infinity, "18", "18px; background:url(https://tracker.invalid)"]) {
      expect(parseEmailMinimumFontSize(value)).toBe(0);
    }
  });
  it("reads <font size> absolute and relative values within the legacy scale without clamping", () => {
    const { min, max, relativeBase } = EMAIL_LEGACY_FONT_SIZE;
    expect(sanitizeHtmlFontSize(String(min - 1))).toBeNull();
    expect(sanitizeHtmlFontSize(String(min))).toBe(String(min));
    expect(sanitizeHtmlFontSize(String(min + 1))).toBe(String(min + 1));
    expect(sanitizeHtmlFontSize(String(max - 1))).toBe(String(max - 1));
    expect(sanitizeHtmlFontSize(String(max))).toBe(String(max));
    expect(sanitizeHtmlFontSize(String(max + 1))).toBeNull();
    expect(sanitizeHtmlFontSize(`+${max - relativeBase}`)).toBe(String(max));
    expect(sanitizeHtmlFontSize(`+${max - relativeBase + 1}`)).toBeNull();
    expect(sanitizeHtmlFontSize(`-${relativeBase - min}`)).toBe(String(min));
    expect(sanitizeHtmlFontSize(`-${relativeBase - min + 1}`)).toBeNull();
    expect(sanitizeHtmlFontSize(" 2px")).toBe("2");
    expect(sanitizeHtmlFontSize("2;color:red")).toBe("2");
    for (const value of ["", "+", "large", "x2", "99999999999999999999"]) {
      expect([value, sanitizeHtmlFontSize(value)]).toEqual([value, null]);
    }
  });

  it("preserves declared image widths only up to the preserved-width limit", () => {
    const max = EMAIL_IMAGE_LIMITS.maxPreservedImageWidthPx;
    expect(preservedImageMinWidth(null, String(max - 1))).toBe(`${max - 1}px`);
    expect(preservedImageMinWidth(null, String(max))).toBe(`${max}px`);
    expect(preservedImageMinWidth(null, String(max + 1))).toBeNull();
    expect(preservedImageMinWidth(`${max - 1}px`, null)).toBe(`${max - 1}px`);
    expect(preservedImageMinWidth(`${max}px`, null)).toBe(`${max}px`);
    expect(preservedImageMinWidth(`${max + 1}px`, null)).toBeNull();
    expect(preservedImageMinWidth(null, "17.5")).toBe("17.5px");
    // An inline CSS width overrides the attribute, so it alone decides.
    expect(preservedImageMinWidth(`${max + 1}px`, "18")).toBeNull();
    expect(preservedImageMinWidth("50%", "18")).toBeNull();
    expect(preservedImageMinWidth("auto", "18")).toBeNull();
    expect(preservedImageMinWidth("18", null)).toBeNull();
    for (const value of ["0", "10%", "2em", "-4", "calc(10px)", ""]) {
      expect(preservedImageMinWidth(null, value)).toBeNull();
    }
  });

  it("grows the message frame with its content up to the frame-height limit", () => {
    const limit = EMAIL_CSS_LIMITS.maxFrameHeightPx;
    expect(emailFrameHeight(limit - 1)).toBe(limit - 1);
    expect(emailFrameHeight(limit)).toBe(limit);
    expect(emailFrameHeight(limit + 1)).toBe(limit);
    expect(emailFrameHeight(0)).toBe(0);
    expect(emailFrameHeight(-5)).toBe(0);
    expect(emailFrameHeight(Number.NaN)).toBe(0);
  });

  it("uses the same absolute and percentage boundaries for CSS and HTML dimensions", () => {
    expect(sanitizeCssDeclaration("width", "0px")).toBe("0px");
    expect(sanitizeCssDeclaration("width", `${EMAIL_CSS_LIMITS.maxAbsolutePx}px`)).toBe(`${EMAIL_CSS_LIMITS.maxAbsolutePx}px`);
    expect(sanitizeCssDeclaration("width", `${EMAIL_CSS_LIMITS.maxAbsolutePx + 1}px`)).toBeNull();
    for (const [property, color] of [["border-left", "rgb(221, 221, 221)"], ["border", "#ddd"]]) {
      expect(sanitizeCssDeclaration(property, `${EMAIL_CSS_LIMITS.maxAbsolutePx - 1}px solid ${color}`)).toBe(`${EMAIL_CSS_LIMITS.maxAbsolutePx - 1}px solid ${color}`);
      expect(sanitizeCssDeclaration(property, `${EMAIL_CSS_LIMITS.maxAbsolutePx}px solid ${color}`)).toBe(`${EMAIL_CSS_LIMITS.maxAbsolutePx}px solid ${color}`);
      expect(sanitizeCssDeclaration(property, `${EMAIL_CSS_LIMITS.maxAbsolutePx + 1}px solid ${color}`)).toBeNull();
    }
    expect(sanitizeHtmlDimension(String(EMAIL_CSS_LIMITS.maxAbsolutePx), false)).toBe(String(EMAIL_CSS_LIMITS.maxAbsolutePx));
    expect(sanitizeHtmlDimension(String(EMAIL_CSS_LIMITS.maxAbsolutePx + 1), false)).toBeNull();
    expect(sanitizeHtmlDimension(`${EMAIL_CSS_LIMITS.maxAbsolutePx - 1}px`, false)).toBe(String(EMAIL_CSS_LIMITS.maxAbsolutePx - 1));
    expect(sanitizeHtmlDimension(`${EMAIL_CSS_LIMITS.maxAbsolutePx}PX`, false)).toBe(String(EMAIL_CSS_LIMITS.maxAbsolutePx));
    expect(sanitizeHtmlDimension(`${EMAIL_CSS_LIMITS.maxAbsolutePx + 1}px`, false)).toBeNull();
    expect(sanitizeHtmlDimension(`${EMAIL_CSS_LIMITS.maxPercentage}%`, true)).toBe(`${EMAIL_CSS_LIMITS.maxPercentage}%`);
    expect(sanitizeHtmlDimension(`${EMAIL_CSS_LIMITS.maxPercentage + 1}%`, true)).toBeNull();
    expect(sanitizeHtmlDimension("50%", false)).toBeNull();
    expect(sanitizeHtmlCellPadding(String(EMAIL_CSS_LIMITS.maxAbsolutePx))).toBe(String(EMAIL_CSS_LIMITS.maxAbsolutePx));
    expect(sanitizeHtmlCellPadding(`${EMAIL_CSS_LIMITS.maxAbsolutePx}px`)).toBe(String(EMAIL_CSS_LIMITS.maxAbsolutePx));
    expect(sanitizeHtmlCellPadding(String(EMAIL_CSS_LIMITS.maxAbsolutePx + 1))).toBeNull();
  });

  // Expected values were measured from WebKit and Chromium computed styles:
  // legacy attributes keep their leading number and ignore the rest.
  it("reads width and height attributes the way browsers do", () => {
    const cases: Array<[string, string | null]> = [
      ["20", "20"], ["20px", "20"], ["20PX", "20"], ["20pt", "20"], ["20em", "20"], ["2vw", "2"],
      ["20 px", "20"], [" 20", "20"], ["\t20", "20"], ["\n20", "20"], ["\f20", "20"], ["\r20", "20"], ["20\t", "20"],
      ["20.", "20"], ["20.5", "20.5"], ["20.50px", "20.5"], ["183.207", "183.207"], ["020", "20"], ["0", "0"], ["0px", "0"],
      ["1e2", "1"], ["20.5.3", "20.5"], ["20px;color:red", "20"],
      ["20%", "20%"], ["20.%", "20%"], ["20.5%", "20.5%"], ["20%px", "20%"], ["20 %", "20"],
      [" 20", null], ["+20", null], ["-20", null], ["-20px", null], [".5", null], ["px", null], ["", null],
      ["auto", null], ["calc(20px)", null], ["２０", null],
    ];
    for (const [value, expected] of cases) {
      expect([value, sanitizeHtmlDimension(value, true)]).toEqual([value, expected]);
    }
    expect(sanitizeHtmlDimension("20%", false)).toBeNull();
    expect(sanitizeHtmlDimension("20.%", false)).toBeNull();
    expect(sanitizeHtmlDimension("20 %", false)).toBe("20");
  });

  it("reads cellpadding the way browsers do, including their zero fallback", () => {
    const cases: Array<[string, string | null]> = [
      ["20", "20"], ["20px", "20"], ["+20", "20"], ["20.9", "20"], ["20.9%", "20"], ["20%", "20"], ["007", "7"],
      ["\n20", "20"], ["20e1", "20"], ["0", "0"],
      // Browsers render these as zero padding rather than the 1px default.
      ["-20", "0"], ["-0", "0"], [" -5", "0"], ["+", "0"], ["-", "0"], ["+-1", "0"], ["auto", "0"], [" ", "0"], ["２０", "0"],
      // Only an empty attribute falls back to the default.
      ["", null],
    ];
    for (const [value, expected] of cases) {
      expect([value, sanitizeHtmlCellPadding(value)]).toEqual([value, expected]);
    }
    expect(sanitizeCssDeclaration("width", `${EMAIL_CSS_LIMITS.maxPercentage}%`)).toBe(`${EMAIL_CSS_LIMITS.maxPercentage}%`);
    expect(sanitizeCssDeclaration("width", `${EMAIL_CSS_LIMITS.maxPercentage + 1}%`)).toBeNull();
    expect(sanitizeCssDeclaration("height", "auto")).toBe("auto");
    expect(sanitizeCssDeclaration("border", "0")).toBe("0");
  });

  it("bounds relative lengths, font sizes, and unitless line heights without clamping", () => {
    expect(sanitizeCssDeclaration("padding", `${EMAIL_CSS_LIMITS.maxRelativeLength}em`)).toBe(`${EMAIL_CSS_LIMITS.maxRelativeLength}em`);
    expect(sanitizeCssDeclaration("padding", `${EMAIL_CSS_LIMITS.maxRelativeLength + 1}em`)).toBeNull();
    expect(sanitizeCssDeclaration("font-size", `${EMAIL_CSS_LIMITS.maxFontSizePx}px`)).toBe(`${EMAIL_CSS_LIMITS.maxFontSizePx}px`);
    expect(sanitizeCssDeclaration("font-size", `${EMAIL_CSS_LIMITS.maxFontSizePx + 1}px`)).toBeNull();
    expect(sanitizeCssDeclaration("font-size", `${EMAIL_CSS_LIMITS.maxFontSizePercentage}%`)).toBe(`${EMAIL_CSS_LIMITS.maxFontSizePercentage}%`);
    expect(sanitizeCssDeclaration("font-size", `${EMAIL_CSS_LIMITS.maxFontSizePercentage + 1}%`)).toBeNull();
    expect(sanitizeCssDeclaration("font", "14px Arial")).toBe("14px arial");
    // Zero is the lower bound, with or without a unit; nothing below it passes.
    expect(sanitizeCssDeclaration("font-size", "0")).toBe("0");
    expect(sanitizeCssDeclaration("font-size", "0px")).toBe("0px");
    expect(sanitizeCssDeclaration("font-size", "-0")).toBeNull();
    expect(sanitizeCssDeclaration("font-size", "-1px")).toBeNull();
    expect(sanitizeCssDeclaration("font-size", "00")).toBeNull();
    expect(sanitizeCssDeclaration("font-size", "1")).toBeNull();
    expect(sanitizeCssDeclaration("font", "0 Arial")).toBe("0 arial");
    expect(sanitizeCssDeclaration("font", "0/0px Arial")).toBe("0/0px arial");
    expect(sanitizeCssDeclaration("font", "0.5em Arial")).toBe("0.5em arial");
    expect(sanitizeCssDeclaration("font", "05 Arial")).toBeNull();
    expect(sanitizeCssDeclaration("line-height", String(EMAIL_CSS_LIMITS.maxLineHeightUnitless))).toBe(String(EMAIL_CSS_LIMITS.maxLineHeightUnitless));
    expect(sanitizeCssDeclaration("line-height", String(EMAIL_CSS_LIMITS.maxLineHeightUnitless + 1))).toBeNull();
  });

  it("bounds a percentage line-height by its own font-relative limit, not the layout percentage", () => {
    // A percentage line-height multiplies the font size, like a unitless one;
    // the 100% layout cap would reject ordinary spacing such as 150%.
    expect(sanitizeCssDeclaration("line-height", "150%")).toBe("150%");
    const limit = EMAIL_CSS_LIMITS.maxLineHeightPercentage;
    expect(sanitizeCssDeclaration("line-height", `${limit - 1}%`)).toBe(`${limit - 1}%`);
    expect(sanitizeCssDeclaration("line-height", `${limit}%`)).toBe(`${limit}%`);
    expect(sanitizeCssDeclaration("line-height", `${limit + 1}%`)).toBeNull();
    expect(sanitizeCssDeclaration("font", "14px/150% Arial")).toBe("14px/150% arial");
    expect(sanitizeCssDeclaration("font", `14px/${limit + 1}% Arial`)).toBeNull();
    expect(sanitizeCssDeclaration("line-height", "-150%")).toBeNull();
    // Layout percentages keep the tighter cap.
    expect(sanitizeCssDeclaration("width", "150%")).toBeNull();
  });

  it("allows bounded negative margins but rejects them for padding and fixed overlays", () => {
    expect(sanitizeCssDeclaration("margin-top", "-12px")).toBe("-12px");
    expect(sanitizeCssDeclaration("padding-top", "-12px")).toBeNull();
    expect(sanitizeCssDeclaration("position", "fixed")).toBeNull();
    expect(sanitizeCssDeclaration("position", "absolute")).toBe("absolute");
    expect(sanitizeCssDeclaration("left", "-50%")).toBe("-50%");
    expect(sanitizeCssDeclaration("left", "-101%")).toBeNull();
  });

  it("accepts auto on margin longhands as the shorthand does, and nothing else new", () => {
    for (const side of ["top", "right", "bottom", "left"]) {
      expect(sanitizeCssDeclaration(`margin-${side}`, "auto")).toBe("auto");
      expect(sanitizeCssDeclaration(`margin-${side}`, " AUTO ")).toBe("auto");
      expect(sanitizeCssDeclaration(`margin-${side}`, "auto auto")).toBeNull();
      expect(sanitizeCssDeclaration(`margin-${side}`, "calc(100% - 10px)")).toBeNull();
      expect(sanitizeCssDeclaration(`margin-${side}`, "url(https://tracker.invalid/x.gif)")).toBeNull();
      expect(sanitizeCssDeclaration(`margin-${side}`, "expression(alert(1))")).toBeNull();
      expect(sanitizeCssDeclaration(`margin-${side}`, "-101%")).toBeNull();
    }
    expect(sanitizeCssDeclaration("margin", "0 auto")).toBe("0 auto");
  });

  it("bounds companion flex factors and cosmetic opacity values", () => {
    expect(sanitizeCssDeclaration("flex-grow", "16")).toBe("16");
    expect(sanitizeCssDeclaration("flex-grow", "17")).toBeNull();
    expect(sanitizeCssDeclaration("flex", "1 1 320px")).toBe("1 1 320px");
    expect(sanitizeCssDeclaration("flex", "17 1 320px")).toBeNull();
    expect(sanitizeCssDeclaration("opacity", "1")).toBe("1");
    expect(sanitizeCssDeclaration("opacity", "1.1")).toBeNull();
  });

  it("keeps the cosmetic list-marker family so a sender's marker reset survives", () => {
    // Senders repurpose <ol>/<ul> as layout scaffolding and suppress the
    // markers with their own CSS; dropping that reset leaks the UA's default
    // decimal/disc markers (the "1." "2." other clients don't show).
    expect(sanitizeCssDeclaration("list-style", "none")).toBe("none");
    expect(sanitizeCssDeclaration("list-style-type", "none")).toBe("none");
    expect(sanitizeCssDeclaration("list-style", "square inside")).toBe("square inside");
    expect(sanitizeCssDeclaration("list-style", "none inside none")).toBe("none inside none");
    expect(sanitizeCssDeclaration("list-style-type", "lower-roman")).toBe("lower-roman");
    expect(sanitizeCssDeclaration("list-style-position", "outside")).toBe("outside");
    expect(sanitizeCssDeclaration("list-style-image", "none")).toBe("none");
    // Malformed / out-of-family values are rejected rather than clamped.
    expect(sanitizeCssDeclaration("list-style-type", "bogus-glyph")).toBeNull();
    expect(sanitizeCssDeclaration("list-style-position", "floating")).toBeNull();
    expect(sanitizeCssDeclaration("list-style", "square inside circle")).toBeNull();
  });

  it("keeps the background-layout family so a background image is placed and scaled inside its box", () => {
    expect(sanitizeCssDeclaration("background-size", "48px 48px")).toBe("48px 48px");
    expect(sanitizeCssDeclaration("background-size", "cover")).toBe("cover");
    expect(sanitizeCssDeclaration("background-size", "contain")).toBe("contain");
    expect(sanitizeCssDeclaration("background-size", "auto 50%")).toBe("auto 50%");
    expect(sanitizeCssDeclaration("background-position", "center center")).toBe("center center");
    expect(sanitizeCssDeclaration("background-position", "50% 50%")).toBe("50% 50%");
    expect(sanitizeCssDeclaration("background-position", "right 8px bottom -4px")).toBe("right 8px bottom -4px");
    expect(sanitizeCssDeclaration("background-repeat", "no-repeat")).toBe("no-repeat");
    expect(sanitizeCssDeclaration("background-repeat", "repeat no-repeat")).toBe("repeat no-repeat");
    expect(sanitizeCssDeclaration("background-origin", "content-box")).toBe("content-box");
    expect(sanitizeCssDeclaration("background-clip", "padding-box")).toBe("padding-box");
    // Malformed / out-of-family values are rejected rather than clamped.
    expect(sanitizeCssDeclaration("background-size", "cover contain")).toBeNull();
    expect(sanitizeCssDeclaration("background-size", "-48px")).toBeNull();
    expect(sanitizeCssDeclaration("background-size", "48px 48px 48px")).toBeNull();
    expect(sanitizeCssDeclaration("background-size", "48px, 24px")).toBeNull();
    expect(sanitizeCssDeclaration("background-position", "center center, left top")).toBeNull();
    expect(sanitizeCssDeclaration("background-position", "left top left top left")).toBeNull();
    expect(sanitizeCssDeclaration("background-position", "calc(50% - 4px)")).toBeNull();
    expect(sanitizeCssDeclaration("background-repeat", "repeat-x repeat")).toBeNull();
    expect(sanitizeCssDeclaration("background-clip", "text")).toBeNull();
    expect(sanitizeCssDeclaration("background-attachment", "fixed")).toBeNull();
  });

  it("bounds background sizes and offsets by the shared absolute and percentage limits", () => {
    const { maxAbsolutePx, maxPercentage, maxRelativeLength } = EMAIL_CSS_LIMITS;
    for (const property of ["background-size", "background-position"]) {
      expect(sanitizeCssDeclaration(property, `${maxAbsolutePx - 1}px`)).toBe(`${maxAbsolutePx - 1}px`);
      expect(sanitizeCssDeclaration(property, `${maxAbsolutePx}px`)).toBe(`${maxAbsolutePx}px`);
      expect(sanitizeCssDeclaration(property, `${maxAbsolutePx + 1}px`)).toBeNull();
      expect(sanitizeCssDeclaration(property, `${maxPercentage - 1}%`)).toBe(`${maxPercentage - 1}%`);
      expect(sanitizeCssDeclaration(property, `${maxPercentage}%`)).toBe(`${maxPercentage}%`);
      expect(sanitizeCssDeclaration(property, `${maxPercentage + 1}%`)).toBeNull();
      expect(sanitizeCssDeclaration(property, `${maxRelativeLength}em`)).toBe(`${maxRelativeLength}em`);
      expect(sanitizeCssDeclaration(property, `${maxRelativeLength + 1}em`)).toBeNull();
    }
    expect(sanitizeCssDeclaration("background-position", `-${maxAbsolutePx}px`)).toBe(`-${maxAbsolutePx}px`);
    expect(sanitizeCssDeclaration("background-position", `-${maxAbsolutePx + 1}px`)).toBeNull();
  });

  it("never admits a remote image through the background-layout family", () => {
    for (const property of ["background-size", "background-position", "background-repeat", "background-origin", "background-clip"]) {
      expect(sanitizeCssDeclaration(property, "url(https://tracker.invalid/pixel.gif)")).toBeNull();
    }
    expect(sanitizeCssDeclaration("background", "url(https://tracker.invalid/pixel.gif) center/48px no-repeat")).toBeNull();
  });

  it("never admits a remote marker image through the list-style family", () => {
    // The shared url() ban must hold here too: no list property may fetch.
    expect(sanitizeCssDeclaration("list-style-image", "url(https://tracker.invalid/dot.gif)")).toBeNull();
    expect(sanitizeCssDeclaration("list-style", "url(https://tracker.invalid/dot.gif)")).toBeNull();
    expect(sanitizeCssDeclaration("list-style", "disc url(https://tracker.invalid/dot.gif)")).toBeNull();
  });
});
