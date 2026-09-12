import { describe, expect, it } from "vitest";
import { commands, isEditableTarget, matchesShortcut, shortcutSteps } from "./commands";

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

  it("registers Superhuman folder chords for matching destinations", () => {
    expect(commands.find((command) => command.id === "mailbox.inbox")?.keys).toEqual(["g then i"]);
    expect(commands.find((command) => command.id === "drafts.open")?.keys).toEqual(["g then d"]);
    expect(commands.find((command) => command.id === "labels.open")?.keys).toEqual(["l"]);
  });

  it("splits sequential shortcuts into independently matchable steps", () => {
    const [prefix, destination] = shortcutSteps("g then d");
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "g" }), prefix)).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "d" }), destination)).toBe(true);
  });
});

it("distinguishes reply from refresh and matches the send modifier", () => {
  expect(matchesShortcut(new KeyboardEvent("keydown", { key: "r" }), "Shift+r")).toBe(false);
  expect(matchesShortcut(new KeyboardEvent("keydown", { key: "R", shiftKey: true }), "Shift+r")).toBe(true);
  expect(matchesShortcut(new KeyboardEvent("keydown", { key: "R", shiftKey: true }), "r")).toBe(false);
  expect(matchesShortcut(new KeyboardEvent("keydown", { key: "Enter", metaKey: true }), "Mod+Enter")).toBe(true);
  expect(matchesShortcut(new KeyboardEvent("keydown", { key: "Enter" }), "Mod+Enter")).toBe(false);
});

it("matches desktop font-size shortcuts", () => {
  expect(matchesShortcut(new KeyboardEvent("keydown", { key: "=", metaKey: true }), "Mod+=")).toBe(true);
  expect(matchesShortcut(new KeyboardEvent("keydown", { key: "+", shiftKey: true, ctrlKey: true }), "Mod++")).toBe(true);
  expect(matchesShortcut(new KeyboardEvent("keydown", { key: "-", ctrlKey: true }), "Mod+-")).toBe(true);
  expect(matchesShortcut(new KeyboardEvent("keydown", { key: "=" }), "Mod+=")).toBe(false);
});
