import { describe, expect, it, vi } from "vitest";
import {
  accountCommand,
  commands,
  isEditableTarget,
  matchesShortcut,
  shortcutSteps,
  showAllAccountsCommand,
  type CommandContext,
} from "./commands";

function noopContext(): CommandContext {
  return {
    mailbox: "inbox",
    selectedId: null,
    selectedArchived: false,
    selectedTrashed: false,
    canUnsubscribe: false,
    composerActive: false,
    canUndoSend: false,
    compose: () => {},
    reply: () => {},
    replyAll: () => {},
    forward: () => {},
    openInbox: () => {},
    openAllMail: () => {},
    openTrash: () => {},
    openDrafts: () => {},
    openOutbox: () => {},
    sendDraft: () => {},
    attachFiles: () => {},
    undoSend: () => {},
    selectNext: () => {},
    selectPrevious: () => {},
    archiveSelected: async () => ({}),
    markNotDoneSelected: async () => ({}),
    unsubscribeSelected: () => {},
    trashSelected: async () => ({}),
    restoreSelected: async () => ({}),
    markSpamSelected: async () => ({}),
    setLabelSelected: async () => ({}),
    toggleReadSelected: async () => ({}),
    toggleStarSelected: async () => ({}),
    toggleCheckedSelected: () => {},
    focusSearch: () => {},
    refresh: () => {},
    openDiagnostics: () => {},
    openLabels: () => {},
    openPalette: () => {},
    openShortcutHelp: () => {},
    openSettings: () => {},
    increaseFontSize: () => {},
    decreaseFontSize: () => {},
    canUndoAction: false,
    undoLastAction: () => {},
    showAllAccounts: () => {},
    switchAccount: () => {},
  };
}

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
    expect(commands.find((command) => command.id === "mailbox.allMail")?.keys).toEqual(["g then a"]);
    expect(commands.find((command) => command.id === "mailbox.trash")?.keys).toEqual(["g then t"]);
    expect(commands.find((command) => command.id === "drafts.open")?.keys).toEqual(["g then d"]);
    expect(commands.find((command) => command.id === "labels.open")?.keys).toEqual(["l"]);
  });

  it("gates triage and reply commands to thread-based mailboxes", () => {
    const context = noopContext();
    context.selectedId = "thread-1";
    context.mailbox = "drafts";
    expect(commands.find((command) => command.id === "thread.archive")?.enabled(context)).toBe(false);
    expect(commands.find((command) => command.id === "thread.trash")?.enabled(context)).toBe(false);
    expect(commands.find((command) => command.id === "draft.reply")?.enabled(context)).toBe(false);
    context.mailbox = "inbox";
    expect(commands.find((command) => command.id === "thread.archive")?.enabled(context)).toBe(true);
  });

  it("gates the trash/restore toggle on whether the selected thread is already trashed", () => {
    const context = noopContext();
    context.selectedId = "thread-1";
    expect(commands.find((command) => command.id === "thread.trash")?.enabled(context)).toBe(true);
    expect(commands.find((command) => command.id === "thread.untrash")?.enabled(context)).toBe(false);
    context.selectedTrashed = true;
    expect(commands.find((command) => command.id === "thread.trash")?.enabled(context)).toBe(false);
    expect(commands.find((command) => command.id === "thread.untrash")?.enabled(context)).toBe(true);
  });

  it("registers the conversation triage shortcuts", () => {
    expect(commands.find((command) => command.id === "thread.archive")?.keys).toEqual(["e"]);
    expect(commands.find((command) => command.id === "thread.unarchive")?.keys).toEqual(["Shift+e"]);
    expect(commands.find((command) => command.id === "thread.star")?.keys).toEqual(["s"]);
    expect(commands.find((command) => command.id === "thread.read")?.keys).toEqual(["u"]);
    expect(commands.find((command) => command.id === "thread.trash")?.keys).toEqual(["#"]);
    expect(commands.find((command) => command.id === "thread.spam")?.keys).toEqual(["!"]);
    expect(commands.find((command) => command.id === "thread.unsubscribe")?.keys).toEqual(["Mod+u"]);
  });

  it("matches unsubscribe only with the desktop modifier", () => {
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "u", metaKey: true }), "Mod+u")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "u", ctrlKey: true }), "Mod+u")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "u" }), "Mod+u")).toBe(false);
  });

  it("matches the displayed hash shortcut on US and symbol-producing keyboards", () => {
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "#", shiftKey: true, code: "Digit3" }), "#")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "3", shiftKey: true, code: "Digit3" }), "#")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "3", code: "Digit3" }), "#")).toBe(false);
  });

  it("matches the spam shortcut on US and symbol-producing keyboards", () => {
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "!", shiftKey: true, code: "Digit1" }), "!")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "1", shiftKey: true, code: "Digit1" }), "!")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "1", code: "Digit1" }), "!")).toBe(false);
  });

  it("registers the common shortcut-help key", () => {
    expect(commands.find((command) => command.id === "shortcuts.open")?.keys).toEqual(["?"]);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "?", shiftKey: true }), "?")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "/", shiftKey: true }), "?")).toBe(true);
  });

  it("registers both last-action undo shortcuts", () => {
    const undo = commands.find((command) => command.id === "action.undo");
    expect(undo?.keys).toEqual(["z", "Mod+z"]);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "Z" }), "z")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "z", ctrlKey: true }), "Mod+z")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "z", metaKey: true }), "Mod+z")).toBe(true);
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

describe("account commands", () => {
  it("binds Mod+1..Mod+9 by sort order and leaves later accounts keyless", () => {
    expect(accountCommand("a@example.com", 0).keys).toEqual(["Mod+1"]);
    expect(accountCommand("b@example.com", 8).keys).toEqual(["Mod+9"]);
    expect(accountCommand("c@example.com", 9).keys).toEqual([]);
  });

  it("switches to the given account when run", async () => {
    const context = noopContext();
    context.switchAccount = vi.fn();
    await accountCommand("you@example.com", 0).run(context);
    expect(context.switchAccount).toHaveBeenCalledWith("you@example.com");
  });

  it("show all accounts has no default key and calls showAllAccounts when run", async () => {
    const context = noopContext();
    context.showAllAccounts = vi.fn();
    const command = showAllAccountsCommand();
    expect(command.keys).toEqual([]);
    await command.run(context);
    expect(context.showAllAccounts).toHaveBeenCalledTimes(1);
  });

  it("does not collide with the static command registry's shortcut keys", () => {
    const staticKeys = new Set(commands.flatMap((command) => command.keys.map((key) => key.toLowerCase())));
    for (let index = 0; index < 9; index++) {
      const [key] = accountCommand(`account-${index}@example.com`, index).keys;
      if (key) expect(staticKeys.has(key.toLowerCase())).toBe(false);
    }
  });
});

it("matches desktop font-size shortcuts", () => {
  expect(matchesShortcut(new KeyboardEvent("keydown", { key: "=", metaKey: true }), "Mod+=")).toBe(true);
  expect(matchesShortcut(new KeyboardEvent("keydown", { key: "=", shiftKey: true, ctrlKey: true }), "Mod+=")).toBe(true);
  expect(matchesShortcut(new KeyboardEvent("keydown", { key: "+", shiftKey: true, ctrlKey: true }), "Mod++")).toBe(true);
  expect(matchesShortcut(new KeyboardEvent("keydown", { key: "-", ctrlKey: true }), "Mod+-")).toBe(true);
  expect(matchesShortcut(new KeyboardEvent("keydown", { key: "=" }), "Mod+=")).toBe(false);
});
