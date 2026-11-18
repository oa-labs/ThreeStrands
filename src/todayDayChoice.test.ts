import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";
import { activeDayChoice, clearDayChoice, localDay, readDayChoice, TODAY_DAY_KEY, writeDayChoice } from "./todayDayChoice";
import { readExportablePreferences } from "./userPreferences";

const TODAY = "2026-11-18";

afterEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

describe("the remembered Today day", () => {
  it("reads back a day chosen today and clears when Today is chosen", () => {
    expect(readDayChoice(TODAY)).toBeNull();
    writeDayChoice({ chosenOn: TODAY, day: "2026-11-20" });
    expect(readDayChoice(TODAY)).toEqual({ chosenOn: TODAY, day: "2026-11-20" });
    clearDayChoice();
    expect(readDayChoice(TODAY)).toBeNull();
  });

  it("ignores a choice stamped on an earlier day", () => {
    writeDayChoice({ chosenOn: "2026-11-17", day: "2026-11-20" });
    expect(readDayChoice(TODAY)).toBeNull();
  });

  it("drops the choice at rollover, so the new today shows", () => {
    const choice = { chosenOn: TODAY, day: "2026-11-20" };
    expect(activeDayChoice(choice, TODAY)).toEqual(choice);
    expect(activeDayChoice(choice, "2026-11-19")).toBeNull();
    expect(activeDayChoice(null, TODAY)).toBeNull();
    // Choosing today itself is no choice.
    expect(activeDayChoice({ chosenOn: TODAY, day: TODAY }, TODAY)).toBeNull();
  });

  it("reads corrupt or malformed storage as today", () => {
    for (const raw of ["{not json", "null", "[]", JSON.stringify({ chosenOn: TODAY }), JSON.stringify({ chosenOn: TODAY, day: "tomorrow" }), JSON.stringify({ chosenOn: TODAY, day: 5 })]) {
      localStorage.setItem(TODAY_DAY_KEY, raw);
      expect(readDayChoice(TODAY)).toBeNull();
    }
  });

  it("never throws when storage is blocked", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("blocked"); });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("blocked"); });
    vi.spyOn(Storage.prototype, "removeItem").mockImplementation(() => { throw new Error("blocked"); });
    expect(readDayChoice(TODAY)).toBeNull();
    expect(() => writeDayChoice({ chosenOn: TODAY, day: "2026-11-20" })).not.toThrow();
    expect(() => clearDayChoice()).not.toThrow();
  });

  it("parses a stored day as a local date and rejects impossible ones", () => {
    expect(localDay("2026-11-20")).toEqual(new Date(2026, 10, 20));
    expect(localDay("2026-02-30")).toBeNull();
    expect(localDay("nope")).toBeNull();
  });

  it("is not a synced or exported preference: the settings export neither reads nor carries it, and the transfer code never names it", () => {
    const before = JSON.stringify(readExportablePreferences());
    writeDayChoice({ chosenOn: TODAY, day: "2026-11-20" });
    expect(JSON.stringify(readExportablePreferences())).toBe(before);
    for (const file of ["src-tauri/src/transfer.rs", "src/userPreferences.ts", "src/DataTransferSettings.tsx"]) {
      expect(readFileSync(file, "utf8")).not.toContain(TODAY_DAY_KEY);
      expect(readFileSync(file, "utf8")).not.toContain("shownDay");
    }
  });
});
