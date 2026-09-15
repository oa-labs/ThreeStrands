import { describe, expect, it } from "vitest";
import { EMAIL_CSS_LIMITS, sanitizeCssDeclaration, sanitizeHtmlDimension } from "./emailRenderingPolicy";

describe("email rendering numeric policy", () => {
  it("uses the same absolute and percentage boundaries for CSS and HTML dimensions", () => {
    expect(sanitizeCssDeclaration("width", `${EMAIL_CSS_LIMITS.maxAbsolutePx}px`)).toBe(`${EMAIL_CSS_LIMITS.maxAbsolutePx}px`);
    expect(sanitizeCssDeclaration("width", `${EMAIL_CSS_LIMITS.maxAbsolutePx + 1}px`)).toBeNull();
    expect(sanitizeHtmlDimension(String(EMAIL_CSS_LIMITS.maxAbsolutePx), false)).toBe(String(EMAIL_CSS_LIMITS.maxAbsolutePx));
    expect(sanitizeHtmlDimension(String(EMAIL_CSS_LIMITS.maxAbsolutePx + 1), false)).toBeNull();
    expect(sanitizeCssDeclaration("width", `${EMAIL_CSS_LIMITS.maxPercentage}%`)).toBe(`${EMAIL_CSS_LIMITS.maxPercentage}%`);
    expect(sanitizeCssDeclaration("width", `${EMAIL_CSS_LIMITS.maxPercentage + 1}%`)).toBeNull();
  });

  it("bounds relative lengths, font sizes, and unitless line heights without clamping", () => {
    expect(sanitizeCssDeclaration("padding", `${EMAIL_CSS_LIMITS.maxRelativeLength}em`)).toBe(`${EMAIL_CSS_LIMITS.maxRelativeLength}em`);
    expect(sanitizeCssDeclaration("padding", `${EMAIL_CSS_LIMITS.maxRelativeLength + 1}em`)).toBeNull();
    expect(sanitizeCssDeclaration("font-size", `${EMAIL_CSS_LIMITS.maxFontSizePx}px`)).toBe(`${EMAIL_CSS_LIMITS.maxFontSizePx}px`);
    expect(sanitizeCssDeclaration("font-size", `${EMAIL_CSS_LIMITS.maxFontSizePx + 1}px`)).toBeNull();
    expect(sanitizeCssDeclaration("line-height", String(EMAIL_CSS_LIMITS.maxLineHeightUnitless))).toBe(String(EMAIL_CSS_LIMITS.maxLineHeightUnitless));
    expect(sanitizeCssDeclaration("line-height", String(EMAIL_CSS_LIMITS.maxLineHeightUnitless + 1))).toBeNull();
  });

  it("allows bounded negative margins but rejects them for padding and fixed overlays", () => {
    expect(sanitizeCssDeclaration("margin-top", "-12px")).toBe("-12px");
    expect(sanitizeCssDeclaration("padding-top", "-12px")).toBeNull();
    expect(sanitizeCssDeclaration("position", "fixed")).toBeNull();
    expect(sanitizeCssDeclaration("position", "absolute")).toBe("absolute");
  });
});
