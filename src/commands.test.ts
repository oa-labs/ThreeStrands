import { describe, expect, it } from "vitest";
import { commands, isEditableTarget, matchesShortcut } from "./commands";

describe("command registry", () => {
  it("keeps shortcut keys unambiguous", () => {
    const keys = commands.flatMap((command) => command.keys.map((key) => key.toLowerCase()));
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("matches shortcuts case-insensitively", () => {
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "J" }), "j")).toBe(true);
  });

  it("does not trigger inbox shortcuts in editable controls", () => {
    expect(isEditableTarget(document.createElement("input"))).toBe(true);
    expect(isEditableTarget(document.createElement("textarea"))).toBe(true);
    expect(isEditableTarget(document.createElement("button"))).toBe(false);
  });
});

it("distinguishes reply from refresh and matches the send modifier", () => {
  expect(matchesShortcut(new KeyboardEvent("keydown", { key: "r" }), "Shift+r")).toBe(false);
  expect(matchesShortcut(new KeyboardEvent("keydown", { key: "R", shiftKey: true }), "Shift+r")).toBe(true);
  expect(matchesShortcut(new KeyboardEvent("keydown", { key: "R", shiftKey: true }), "r")).toBe(false);
  expect(matchesShortcut(new KeyboardEvent("keydown", { key: "Enter", metaKey: true }), "Mod+Enter")).toBe(true);
  expect(matchesShortcut(new KeyboardEvent("keydown", { key: "Enter" }), "Mod+Enter")).toBe(false);
});
