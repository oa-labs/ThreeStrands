import { afterEach, describe, expect, it } from "vitest";
import {
  applyFontFamily,
  DEFAULT_AUTO_READ_DELAY_SECONDS,
  DEFAULT_FONT_FAMILY,
  DEFAULT_LOAD_REMOTE_IMAGES,
  MAX_AUTO_READ_DELAY_SECONDS,
  MIN_AUTO_READ_DELAY_SECONDS,
  readAutoReadDelaySeconds,
  readFontFamily,
  readLoadRemoteImages,
  saveAutoReadDelaySeconds,
  saveFontFamily,
  saveLoadRemoteImages,
} from "./settings";

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

describe("automatic read preference", () => {
  afterEach(() => {
    localStorage.clear();
  });

  it("uses the default and restores a saved delay", () => {
    expect(readAutoReadDelaySeconds()).toBe(DEFAULT_AUTO_READ_DELAY_SECONDS);
    saveAutoReadDelaySeconds(8);
    expect(localStorage.getItem("dispatch.settings.autoReadDelaySeconds")).toBe("8");
    expect(readAutoReadDelaySeconds()).toBe(8);
  });

  it("rounds and clamps invalid delays", () => {
    expect(saveAutoReadDelaySeconds(-4)).toBe(MIN_AUTO_READ_DELAY_SECONDS);
    expect(saveAutoReadDelaySeconds(100)).toBe(MAX_AUTO_READ_DELAY_SECONDS);
    expect(saveAutoReadDelaySeconds(Number.NaN)).toBe(DEFAULT_AUTO_READ_DELAY_SECONDS);
    localStorage.setItem("dispatch.settings.autoReadDelaySeconds", "not-a-number");
    expect(readAutoReadDelaySeconds()).toBe(DEFAULT_AUTO_READ_DELAY_SECONDS);
  });
});

describe("remote image preference", () => {
  afterEach(() => {
    localStorage.clear();
  });

  it("uses the safe default and restores a saved preference", () => {
    expect(readLoadRemoteImages()).toBe(DEFAULT_LOAD_REMOTE_IMAGES);
    saveLoadRemoteImages(true);
    expect(localStorage.getItem("dispatch.settings.loadRemoteImages")).toBe("true");
    expect(readLoadRemoteImages()).toBe(true);
  });

  it("treats invalid stored values as disabled", () => {
    localStorage.setItem("dispatch.settings.loadRemoteImages", "yes");
    expect(readLoadRemoteImages()).toBe(false);
  });
});
