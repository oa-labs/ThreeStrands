import { afterEach, describe, expect, it } from "vitest";
import { applyFontFamily, DEFAULT_FONT_FAMILY, readFontFamily, saveFontFamily } from "./settings";

describe("font family preference", () => {
  afterEach(() => {
    localStorage.clear();
    document.documentElement.style.removeProperty("--font-family");
  });

  it("uses the default and restores a saved preference", () => {
    expect(readFontFamily()).toBe(DEFAULT_FONT_FAMILY);
    saveFontFamily("serif");
    expect(localStorage.getItem("dispatch.settings.fontFamily")).toBe("serif");
    expect(readFontFamily()).toBe("serif");
  });

  it("ignores an invalid stored value", () => {
    localStorage.setItem("dispatch.settings.fontFamily", "comic-sans");
    expect(readFontFamily()).toBe(DEFAULT_FONT_FAMILY);
  });

  it("applies the preference as a root CSS variable", () => {
    applyFontFamily("mono");
    expect(document.documentElement.style.getPropertyValue("--font-family")).toContain("monospace");
  });
});
