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
  readSelectedAccountId,
  readSelectedTabForAccount,
  saveAutoReadDelaySeconds,
  saveFontFamily,
  saveLoadRemoteImages,
  saveSelectedAccountId,
  saveSelectedTabForAccount,
} from "./settings";

describe("font family preference", () => {
  afterEach(() => {
    localStorage.clear();
    document.documentElement.style.removeProperty("--font-family");
  });

  it("uses the default and restores a saved preference", () => {
    expect(readFontFamily()).toBe(DEFAULT_FONT_FAMILY);
    saveFontFamily("Georgia");
    expect(localStorage.getItem("threestrands.settings.fontFamily")).toBe("Georgia");
    expect(readFontFamily()).toBe("Georgia");
  });

  it("migrates preferences from the generic family selector", () => {
    localStorage.setItem("threestrands.settings.fontFamily", "serif");
    expect(readFontFamily()).toBe("Georgia");
    localStorage.setItem("threestrands.settings.fontFamily", "mono");
    expect(readFontFamily()).toBe("Menlo");
  });

  it("ignores an unsafe stored value", () => {
    localStorage.setItem("threestrands.settings.fontFamily", "Font\nInjected");
    expect(readFontFamily()).toBe(DEFAULT_FONT_FAMILY);
  });

  it("applies the preference as a root CSS variable", () => {
    applyFontFamily("Menlo");
    expect(document.documentElement.style.getPropertyValue("--font-family")).toContain('"Menlo"');
  });

  it("quotes arbitrary installed family names for CSS", () => {
    saveFontFamily('Font "Special"');
    expect(document.documentElement.style.getPropertyValue("--font-family"))
      .toContain('"Font \\"Special\\""');
  });
});

describe("automatic read preference", () => {
  afterEach(() => {
    localStorage.clear();
  });

  it("uses the default and restores a saved delay", () => {
    expect(readAutoReadDelaySeconds()).toBe(DEFAULT_AUTO_READ_DELAY_SECONDS);
    saveAutoReadDelaySeconds(8);
    expect(localStorage.getItem("threestrands.settings.autoReadDelaySeconds")).toBe("8");
    expect(readAutoReadDelaySeconds()).toBe(8);
  });

  it("rounds and clamps invalid delays", () => {
    expect(saveAutoReadDelaySeconds(-4)).toBe(MIN_AUTO_READ_DELAY_SECONDS);
    expect(saveAutoReadDelaySeconds(100)).toBe(MAX_AUTO_READ_DELAY_SECONDS);
    expect(saveAutoReadDelaySeconds(Number.NaN)).toBe(DEFAULT_AUTO_READ_DELAY_SECONDS);
    localStorage.setItem("threestrands.settings.autoReadDelaySeconds", "not-a-number");
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
    expect(localStorage.getItem("threestrands.settings.loadRemoteImages")).toBe("true");
    expect(readLoadRemoteImages()).toBe(true);
  });

  it("treats invalid stored values as disabled", () => {
    localStorage.setItem("threestrands.settings.loadRemoteImages", "yes");
    expect(readLoadRemoteImages()).toBe(false);
  });
});

describe("selected account preference", () => {
  afterEach(() => {
    localStorage.clear();
  });

  it("restores a selected account", () => {
    expect(readSelectedAccountId()).toBeNull();
    saveSelectedAccountId("work@example.com");
    expect(localStorage.getItem("threestrands.settings.selectedAccountId")).toBe("work@example.com");
    expect(readSelectedAccountId()).toBe("work@example.com");
  });

  it("persists All accounts explicitly", () => {
    saveSelectedAccountId("work@example.com");
    saveSelectedAccountId(null);
    expect(localStorage.getItem("threestrands.settings.selectedAccountId")).toBe("all");
    expect(readSelectedAccountId()).toBeNull();
  });

  it("ignores an invalid stored account id", () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "work@example.com\ninvalid");
    expect(readSelectedAccountId()).toBeNull();
  });
});

describe("selected mailbox tab preference", () => {
  afterEach(() => {
    localStorage.clear();
  });

  it("has no preference until one is saved", () => {
    expect(readSelectedTabForAccount("work@example.com")).toBeUndefined();
    expect(readSelectedTabForAccount(null)).toBeUndefined();
  });

  it("restores a saved split tab for an account, independent of other accounts", () => {
    saveSelectedTabForAccount("work@example.com", "important");
    saveSelectedTabForAccount("home@example.com", "other");
    expect(readSelectedTabForAccount("work@example.com")).toBe("important");
    expect(readSelectedTabForAccount("home@example.com")).toBe("other");
  });

  it("restores the Inbox tab (stored as null) distinctly from no preference", () => {
    saveSelectedTabForAccount("work@example.com", "important");
    saveSelectedTabForAccount("work@example.com", null);
    expect(readSelectedTabForAccount("work@example.com")).toBeNull();
  });

  it("scopes the merged 'All accounts' view under its own key", () => {
    saveSelectedTabForAccount(null, "important");
    expect(readSelectedTabForAccount(null)).toBe("important");
    expect(readSelectedTabForAccount("work@example.com")).toBeUndefined();
  });

  it("ignores a corrupted stored map", () => {
    localStorage.setItem("threestrands.settings.selectedTabByAccount", "not json");
    expect(readSelectedTabForAccount("work@example.com")).toBeUndefined();
  });
});
