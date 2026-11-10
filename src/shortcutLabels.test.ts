import { describe, expect, it } from "vitest";
import { currentShortcutPlatform, formatShortcut, formatShortcutStep } from "./shortcutLabels";

describe("shortcut labels", () => {
  it.each([
    ["MacIntel", "mac"],
    ["iPad", "mac"],
    ["Win32", "other"],
    ["Linux x86_64", "other"],
    ["", "other"],
  ])("treats %s as %s", (platform, expected) => {
    expect(currentShortcutPlatform(platform)).toBe(expected);
  });

  it("uses macOS menu glyphs and modifier order, with no separators, on a Mac", () => {
    expect(formatShortcutStep("Mod+Shift+L", "mac")).toBe("⇧⌘L");
    expect(formatShortcutStep("Mod+Shift+Enter", "mac")).toBe("⇧⌘↵");
    expect(formatShortcutStep("Mod+d", "mac")).toBe("⌘D");
    expect(formatShortcutStep("Mod+,", "mac")).toBe("⌘,");
    expect(formatShortcutStep("Mod++", "mac")).toBe("⌘+");
    expect(formatShortcutStep("Shift+c", "mac")).toBe("⇧C");
    expect(formatShortcutStep("Alt+Shift+Mod+k", "mac")).toBe("⌥⇧⌘K");
  });

  it("uses Ctrl and + joins elsewhere", () => {
    expect(formatShortcutStep("Mod+Shift+L", "other")).toBe("Ctrl+Shift+L");
    expect(formatShortcutStep("Mod+Shift+Enter", "other")).toBe("Ctrl+Shift+Enter");
    expect(formatShortcutStep("Shift+Mod+a", "other")).toBe("Ctrl+Shift+A");
    expect(formatShortcutStep("Mod++", "other")).toBe("Ctrl++");
    expect(formatShortcutStep("Shift+n", "other")).toBe("Shift+N");
  });

  it("leaves a key pressed alone as typed, since l and Shift+L are different bindings", () => {
    for (const platform of ["mac", "other"] as const) {
      expect(formatShortcutStep("l", platform)).toBe("l");
      expect(formatShortcutStep("#", platform)).toBe("#");
      expect(formatShortcutStep("Escape", platform)).toBe("Esc");
      expect(formatShortcutStep("Tab", platform)).toBe("Tab");
    }
  });

  it("keeps sequences readable as steps", () => {
    expect(formatShortcut("g then c", "mac")).toBe("g then c");
    expect(formatShortcut("g then Shift+i", "other")).toBe("g then Shift+I");
  });

  it("keeps the site's cross-platform form for readers on any platform", () => {
    expect(formatShortcutStep("Mod+Shift+Enter", "any")).toBe("⌘/Ctrl + Shift + Enter");
    expect(formatShortcutStep("Mod+k", "any")).toBe("⌘/Ctrl + k");
  });
});
