import { afterEach, describe, expect, it, vi } from "vitest";
import {
  applyFontFamily,
  DEFAULT_AVAILABILITY_PREFERENCES,
  DEFAULT_AUTO_READ_DELAY_SECONDS,
  DEFAULT_FONT_FAMILY,
  DEFAULT_LOAD_REMOTE_IMAGES,
  MAX_AUTO_READ_DELAY_SECONDS,
  MIN_AUTO_READ_DELAY_SECONDS,
  readAutoReadDelaySeconds,
  readAvailabilityPreferences,
  readFontFamily,
  readEmailMinimumFontSize,
  readLabelUsage,
  readLoadRemoteImages,
  readSelectedAccountId,
  readSelectedMailboxForAccount,
  readSelectedTabForAccount,
  readSnippetUsage,
  recordLabelUsed,
  recordSnippetUsed,
  saveAutoReadDelaySeconds,
  saveAvailabilityPreferences,
  saveFontFamily,
  saveEmailMinimumFontSize,
  saveLoadRemoteImages,
  saveSelectedAccountId,
  saveSelectedMailboxForAccount,
  saveSelectedTabForAccount,
} from "./settings";

describe("availability preferences", () => {
  afterEach(() => localStorage.clear());

  it("defaults to weekday working hours and persists valid changes", () => {
    expect(readAvailabilityPreferences().workingWindows).toHaveLength(5);
    const next = { ...DEFAULT_AVAILABILITY_PREFERENCES, timeZone: "America/New_York", defaultDurationMinutes: 45 };
    saveAvailabilityPreferences(next);
    expect(readAvailabilityPreferences()).toEqual(next);
  });

  it("rejects malformed stored preferences", () => {
    localStorage.setItem("threestrands.settings.availabilityPreferences", JSON.stringify({ timeZone: "", workingWindows: [] }));
    expect(readAvailabilityPreferences()).toEqual(DEFAULT_AVAILABILITY_PREFERENCES);
  });
});

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

describe("selected mailbox preference", () => {
  afterEach(() => localStorage.clear());

  it("stores the current folder independently for each account", () => {
    saveSelectedMailboxForAccount("work@example.com", "outbox");
    saveSelectedMailboxForAccount("home@example.com", "allMail");

    expect(readSelectedMailboxForAccount("work@example.com")).toBe("outbox");
    expect(readSelectedMailboxForAccount("home@example.com")).toBe("allMail");
    expect(readSelectedMailboxForAccount("other@example.com")).toBeUndefined();
  });

  it("ignores corrupted saved folders", () => {
    localStorage.setItem("threestrands.settings.selectedMailboxByAccount", JSON.stringify({ "work@example.com": "not-a-folder" }));
    expect(readSelectedMailboxForAccount("work@example.com")).toBeUndefined();
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

describe("label usage tracking", () => {
  afterEach(() => {
    localStorage.clear();
  });

  it("has no usage until a label is recorded", () => {
    expect(readLabelUsage("work@example.com")).toEqual({});
  });

  it("records a timestamp per label, scoped to its account", () => {
    recordLabelUsed("work@example.com", "Label_1");
    recordLabelUsed("home@example.com", "Label_9");
    const workUsage = readLabelUsage("work@example.com");
    expect(Object.keys(workUsage)).toEqual(["Label_1"]);
    expect(workUsage.Label_1).toBeGreaterThan(0);
    expect(readLabelUsage("home@example.com")).toEqual({ Label_9: expect.any(Number) });
  });

  it("updates the timestamp when the same label is used again", () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(new Date("2026-09-01T10:00:00Z"));
      recordLabelUsed("work@example.com", "Label_1");
      vi.setSystemTime(new Date("2026-09-01T10:05:00Z"));
      recordLabelUsed("work@example.com", "Label_1");
      expect(readLabelUsage("work@example.com").Label_1).toBe(Date.parse("2026-09-01T10:05:00Z"));
    } finally {
      vi.useRealTimers();
    }
  });

  it("ignores a corrupted stored map", () => {
    localStorage.setItem("threestrands.settings.labelUsageByAccount", "not json");
    expect(readLabelUsage("work@example.com")).toEqual({});
  });
});

describe("snippet usage tracking", () => {
  afterEach(() => {
    localStorage.clear();
  });

  it("has no usage until a snippet is recorded", () => {
    expect(readSnippetUsage()).toEqual({});
  });

  it("records a timestamp per snippet, global across accounts", () => {
    recordSnippetUsed("snippet-1");
    recordSnippetUsed("snippet-2");
    const usage = readSnippetUsage();
    expect(Object.keys(usage).sort()).toEqual(["snippet-1", "snippet-2"]);
    expect(usage["snippet-1"]).toBeGreaterThan(0);
  });

  it("updates the timestamp when the same snippet is used again", () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(new Date("2026-09-01T10:00:00Z"));
      recordSnippetUsed("snippet-1");
      vi.setSystemTime(new Date("2026-09-01T10:05:00Z"));
      recordSnippetUsed("snippet-1");
      expect(readSnippetUsage()["snippet-1"]).toBe(Date.parse("2026-09-01T10:05:00Z"));
    } finally {
      vi.useRealTimers();
    }
  });

  it("ignores a corrupted stored map", () => {
    localStorage.setItem("threestrands.settings.snippetUsage", "not json");
    expect(readSnippetUsage()).toEqual({});
  });
});

describe("minimum email font size preference", () => {
  afterEach(() => localStorage.clear());

  it("defaults to off and restores a saved size", () => {
    expect(readEmailMinimumFontSize()).toBe(0);
    expect(saveEmailMinimumFontSize(18)).toBe(18);
    expect(readEmailMinimumFontSize()).toBe(18);
    saveEmailMinimumFontSize(0);
    expect(readEmailMinimumFontSize()).toBe(0);
  });

  it("rejects invalid stored values and handles unavailable storage", () => {
    localStorage.setItem("threestrands.settings.emailMinimumFontSize", "9000");
    expect(readEmailMinimumFontSize()).toBe(0);
    const get = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("unavailable"); });
    expect(readEmailMinimumFontSize()).toBe(0);
    get.mockRestore();
    const set = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("unavailable"); });
    expect(saveEmailMinimumFontSize(18)).toBe(18);
    set.mockRestore();
  });
});
