import { describe, expect, it } from "vitest";
import { commands } from "./commands";
import {
  CHECK_FOR_UPDATES_ID, MENU_LAYOUT, diffEnabled, layoutCommandEntries, menuEnabledStates, menuItemEnabled,
  resolveCommandItem, toAccelerator,
} from "./appMenu";
import { commandContext } from "./test/commandContext";

describe("app menu layout", () => {
  it("references only commands in the registry", () => {
    for (const entry of layoutCommandEntries()) {
      if (entry.id !== CHECK_FOR_UPDATES_ID) expect(commands.map((command) => command.id)).toContain(entry.id);
    }
  });

  it("uses registry titles unless the menu overrides one", () => {
    const byId = (id: string) => commands.find((command) => command.id === id)!;
    const resolved = (id: string) => resolveCommandItem(layoutCommandEntries().find((entry) => entry.id === id)!);
    expect(resolved("draft.reply").title).toBe(byId("draft.reply").title);
    expect(resolved("settings.open").title).toBe("Settings…");
    expect(resolved("labels.open").title).toBe("Labels…");
  });

  it("throws on an unknown command id", () => {
    expect(() => resolveCommandItem({ kind: "command", id: "no.such" })).toThrow(/no\.such/);
  });

  it("gives Settings its Cmd+, accelerator and single-key commands none", () => {
    const accelerator = (id: string) => resolveCommandItem({ kind: "command", id }).accelerator;
    expect(accelerator("settings.open")).toBe("CmdOrCtrl+,");
    expect(accelerator("palette.open")).toBe("CmdOrCtrl+k");
    expect(accelerator("theme.toggle")).toBe("CmdOrCtrl+d");
    expect(accelerator("font.increase")).toBe("CmdOrCtrl+=");
    expect(accelerator("draft.reply")).toBeNull();
    expect(accelerator("mailbox.inbox")).toBeNull();
    expect(accelerator(CHECK_FOR_UPDATES_ID)).toBeNull();
  });

  it("converts Cmd shortcuts and drops chords, single keys, and webview-owned keys", () => {
    expect(toAccelerator("Mod+Shift+L")).toBe("CmdOrCtrl+Shift+L");
    expect(toAccelerator("g then i")).toBeNull();
    expect(toAccelerator("Mod+k then x")).toBeNull();
    expect(toAccelerator("r")).toBeNull();
    expect(toAccelerator("Mod+Enter")).toBeNull();
    expect(toAccelerator("Mod+a")).toBeNull();
    expect(toAccelerator("Mod+z")).toBeNull();
  });

  it("gives no accelerator to any command that is not Cmd-bound in the registry", () => {
    for (const entry of layoutCommandEntries()) {
      const { accelerator } = resolveCommandItem(entry);
      if (accelerator) expect(accelerator.startsWith("CmdOrCtrl+")).toBe(true);
    }
  });

  it("has a Window and a Help section", () => {
    expect(MENU_LAYOUT.map((section) => section.role).filter(Boolean)).toEqual(["window", "help"]);
  });
});

describe("app menu enabled state", () => {
  it("follows each command's enabled predicate", () => {
    const states = menuEnabledStates(commandContext({ selectedId: null }));
    expect(states.get("draft.reply")).toBe(false);
    expect(states.get("draft.new")).toBe(true);
    expect(menuEnabledStates(commandContext({ selectedId: "t1" })).get("draft.reply")).toBe(true);
  });

  it("disables everything while a dialog or the palette holds the keyboard, or the app is closing", () => {
    for (const context of [{ interactionScope: "modal" as const }, { interactionScope: "palette" as const }, { closing: true }]) {
      expect(menuItemEnabled("settings.open", commandContext(context))).toBe(false);
      expect(menuItemEnabled(CHECK_FOR_UPDATES_ID, commandContext(context))).toBe(false);
    }
    expect(menuItemEnabled(CHECK_FOR_UPDATES_ID, commandContext())).toBe(true);
  });

  it("diffs to only the entries that changed", () => {
    const previous = new Map([["a", true], ["b", false], ["c", true]]);
    const next = new Map([["a", true], ["b", true], ["c", false], ["d", true]]);
    expect(diffEnabled(previous, next)).toEqual([["b", true], ["c", false], ["d", true]]);
    expect(diffEnabled(next, next)).toEqual([]);
  });
});
