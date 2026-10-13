import { describe, expect, it } from "vitest";
import { commands, type Command } from "./commands";
import { formattingShortcuts } from "./richText";
import { formatShortcutStep, SHORTCUT_GROUP_ORDER, shortcutDisplaySteps, shortcutGroups } from "./shortcutCatalog";

describe("shortcut catalog", () => {
  it("lists every bound command and formatting shortcut once, in group order", () => {
    const groups = shortcutGroups();
    expect(groups.map((group) => group.group)).toEqual([...SHORTCUT_GROUP_ORDER]);
    const ids = groups.flatMap((group) => group.entries.map((entry) => entry.id));
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids).toEqual(expect.arrayContaining([
      ...commands.filter((command) => command.keys.length > 0).map((command) => command.id),
      ...formattingShortcuts.map((shortcut) => shortcut.id),
    ]));
    expect(ids).not.toContain("gettingStarted.open");
  });

  it("includes extra commands that have keys", () => {
    const extra: Command = { id: "account.1", title: "Switch to Work", keys: ["g then 1"], group: "Navigation", enabled: () => true, run: async () => ({}) };
    const navigation = shortcutGroups([extra]).find((group) => group.group === "Navigation")!;
    expect(navigation.entries.at(-1)).toEqual({ id: "account.1", title: "Switch to Work", keys: ["g then 1"] });
  });

  it("formats modifiers and sequences the way the app displays them", () => {
    expect(formatShortcutStep("Mod+Shift+Enter")).toBe("⌘/Ctrl + Shift + Enter");
    expect(formatShortcutStep("Mod++")).toBe("⌘/Ctrl + +");
    expect(shortcutDisplaySteps("g then i")).toEqual(["g", "i"]);
    expect(shortcutDisplaySteps("Mod+k")).toEqual(["⌘/Ctrl + k"]);
  });
});
