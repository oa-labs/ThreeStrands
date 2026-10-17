import { afterEach, describe, expect, it, vi } from "vitest";
import { MAX_RECENT_SEARCHES, readRecentSearches, recordRecentSearch, RECENT_SEARCHES_KEY } from "./recentSearches";

describe("recent searches", () => {
  afterEach(() => {
    localStorage.removeItem(RECENT_SEARCHES_KEY);
    vi.restoreAllMocks();
  });

  it("keeps the newest first without case-insensitive duplicates", () => {
    recordRecentSearch("invoice");
    recordRecentSearch("from:ana");
    expect(recordRecentSearch("  Invoice ")).toEqual(["Invoice", "from:ana"]);
    expect(readRecentSearches()).toEqual(["Invoice", "from:ana"]);
  });

  it("ignores blank searches and caps the list", () => {
    expect(recordRecentSearch("   ")).toEqual([]);
    for (let index = 0; index < MAX_RECENT_SEARCHES + 3; index += 1) recordRecentSearch(`query ${index}`);
    const stored = readRecentSearches();
    expect(stored).toHaveLength(MAX_RECENT_SEARCHES);
    expect(stored[0]).toBe(`query ${MAX_RECENT_SEARCHES + 2}`);
  });

  it("treats corrupt or foreign data as empty", () => {
    localStorage.setItem(RECENT_SEARCHES_KEY, "{not json");
    expect(readRecentSearches()).toEqual([]);
    localStorage.setItem(RECENT_SEARCHES_KEY, JSON.stringify(["ok", 4, "", null]));
    expect(readRecentSearches()).toEqual(["ok"]);
  });

  it("still returns the updated list when storage refuses writes", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(recordRecentSearch("roadmap")).toEqual(["roadmap"]);
  });

  it("stays out of the settings namespace that transfers between devices", () => {
    expect(RECENT_SEARCHES_KEY.startsWith("threestrands.settings.")).toBe(false);
  });
});
