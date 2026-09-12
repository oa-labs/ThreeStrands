import { afterEach, describe, expect, it } from "vitest";
import {
  applyFontScale,
  changeFontScale,
  DEFAULT_FONT_SCALE,
  FONT_SCALE_KEY,
  MAX_FONT_SCALE,
  MIN_FONT_SCALE,
  readFontScale,
  saveFontScale,
} from "./fontScale";

describe("font scale preference", () => {
  afterEach(() => {
    localStorage.clear();
    document.documentElement.style.removeProperty("--font-scale");
  });

  it("uses the default and restores a saved preference", () => {
    expect(readFontScale()).toBe(DEFAULT_FONT_SCALE);
    saveFontScale(120);
    expect(localStorage.getItem(FONT_SCALE_KEY)).toBe("120");
    expect(readFontScale()).toBe(120);
  });

  it("clamps changes to readable limits", () => {
    expect(changeFontScale(MAX_FONT_SCALE, 1)).toBe(MAX_FONT_SCALE);
    expect(changeFontScale(MIN_FONT_SCALE, -1)).toBe(MIN_FONT_SCALE);
    expect(saveFontScale(1000)).toBe(MAX_FONT_SCALE);
    expect(saveFontScale(-1000)).toBe(MIN_FONT_SCALE);
  });

  it("applies the preference as a root CSS scale", () => {
    applyFontScale(130);
    expect(document.documentElement.style.getPropertyValue("--font-scale")).toBe("1.3");
  });
});
