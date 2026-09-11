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
