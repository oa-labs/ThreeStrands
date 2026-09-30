import { describe, expect, it } from "vitest";
import { EMAIL_CSS_LIMITS, emailFrameHeight, sanitizeCssDeclaration, sanitizeHtmlDimension } from "./emailRenderingPolicy";

describe("email rendering numeric policy", () => {
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
    expect(sanitizeHtmlDimension(String(EMAIL_CSS_LIMITS.maxAbsolutePx), false)).toBe(String(EMAIL_CSS_LIMITS.maxAbsolutePx));
    expect(sanitizeHtmlDimension(String(EMAIL_CSS_LIMITS.maxAbsolutePx + 1), false)).toBeNull();
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

  it("bounds companion flex factors and cosmetic opacity values", () => {
    expect(sanitizeCssDeclaration("flex-grow", "16")).toBe("16");
    expect(sanitizeCssDeclaration("flex-grow", "17")).toBeNull();
    expect(sanitizeCssDeclaration("flex", "1 1 320px")).toBe("1 1 320px");
    expect(sanitizeCssDeclaration("flex", "17 1 320px")).toBeNull();
    expect(sanitizeCssDeclaration("opacity", "1")).toBe("1");
    expect(sanitizeCssDeclaration("opacity", "1.1")).toBeNull();
  });
});
