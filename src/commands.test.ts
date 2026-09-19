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
    canNavigateMessages: false,
    canSendAndMarkDone: false,
    composerActive: false,
    canUndoSend: false,
    compose: () => {},
    reply: () => {},
    replyAll: () => {},
    forward: () => {},
    openInbox: () => {},
    openAllMail: () => {},
    openTrash: () => {},
    openSplitInbox: () => {},
    splitInboxCount: 0,
    goToNextSplitTab: () => {},
    goToPreviousSplitTab: () => {},
    openDrafts: () => {},
    openOutbox: () => {},
    sendDraft: () => {},
    sendAndMarkDone: () => {},
    attachFiles: () => {},
    draftReplyWithAI: () => {},
    undoSend: () => {},
    selectNext: () => {},
    selectPrevious: () => {},
    selectNextMessage: () => {},
    selectPreviousMessage: () => {},
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
    toggleOlderMessagesExpanded: () => {},
    pageMessageDown: () => {},
    pageMessageUp: () => {},
    aiSummaryAvailable: false,
    summarizeSelected: async () => ({}),
    focusSearch: () => {},
    refresh: () => {},
    openLabels: () => {},
    openPalette: () => {},
    openShortcutHelp: () => {},
    openSettings: () => {},
    openToday: () => {},
    openTasks: () => {},
    openActions: () => {},
    newTask: () => {},
    increaseFontSize: () => {},
    decreaseFontSize: () => {},
    canUndoAction: false,
    undoLastAction: () => {},
    showAllAccounts: () => {},
    switchAccount: () => {},
    toggleMessageFilter: () => {},
  };
}

describe("command registry", () => {
  it("keeps diagnostics in Settings instead of exposing a standalone command", () => {
    expect(commands.find((command) => command.id === "diagnostics.open")).toBeUndefined();
  });

  it("keeps shortcut keys unambiguous", () => {
    const keys = commands.flatMap((command) => command.keys.map((key) => key.toLowerCase()));
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("matches shortcuts case-insensitively", () => {
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "J" }), "j")).toBe(true);
  });

  it("registers message navigation on N/P and the horizontal arrows", () => {
    const next = commands.find((command) => command.id === "message.next");
    const previous = commands.find((command) => command.id === "message.previous");
    expect(next?.keys).toEqual(["n", "ArrowRight"]);
    expect(previous?.keys).toEqual(["p", "ArrowLeft"]);
    expect(next?.enabled({ ...noopContext(), canNavigateMessages: true })).toBe(true);
    expect(previous?.enabled({ ...noopContext(), canNavigateMessages: true, composerActive: true })).toBe(false);
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

  it("uses d for tasks and Mod+d for adding a task from the conversation", async () => {
    const openTasks = commands.find((command) => command.id === "tasks.open");
    const newTask = commands.find((command) => command.id === "tasks.new");
    expect(openTasks?.keys).toEqual(["d", "g then k"]);
    expect(newTask?.keys).toEqual(["Mod+d"]);
    const start = vi.fn();
    await newTask?.run({ ...noopContext(), selectedId: "thread-1", newTask: start });
    expect(start).toHaveBeenCalledTimes(1);
  });

  it("binds reply to r and reply all to a", () => {
    expect(commands.find((command) => command.id === "draft.reply")?.keys).toEqual(["r"]);
    expect(commands.find((command) => command.id === "draft.replyAll")?.keys).toEqual(["a"]);
  });

  it("sends and marks done with the shifted send shortcut", () => {
    const command = commands.find((candidate) => candidate.id === "draft.sendAndMarkDone");
    expect(command?.keys).toEqual(["Mod+Shift+Enter"]);
    expect(command?.enabled({ ...noopContext(), composerActive: true, canSendAndMarkDone: true })).toBe(true);
    expect(command?.enabled({ ...noopContext(), composerActive: true })).toBe(false);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "Enter", metaKey: true, shiftKey: true }), "Mod+Shift+Enter")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "Enter", ctrlKey: true, shiftKey: true }), "Mod+Shift+Enter")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "Enter", metaKey: true }), "Mod+Shift+Enter")).toBe(false);
  });

  it("drafts a reply with AI on Mod+J while the composer is open", async () => {
    const command = commands.find((candidate) => candidate.id === "draft.replyAssist");
    expect(command?.keys).toEqual(["Mod+j"]);
    expect(command?.enabled({ ...noopContext(), composerActive: true })).toBe(true);
    expect(command?.enabled(noopContext())).toBe(false);
    const draftReplyWithAI = vi.fn();
    await command?.run({ ...noopContext(), composerActive: true, draftReplyWithAI });
    expect(draftReplyWithAI).toHaveBeenCalledTimes(1);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "j", metaKey: true }), "Mod+j")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "j", ctrlKey: true }), "Mod+j")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "j" }), "Mod+j")).toBe(false);
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

  it("gates AI summarize on a thread selection, thread mailbox, and AI availability", () => {
    const context = noopContext();
    expect(commands.find((command) => command.id === "thread.summarize")?.enabled(context)).toBe(false);
    context.selectedId = "thread-1";
    expect(commands.find((command) => command.id === "thread.summarize")?.enabled(context)).toBe(false);
    context.aiSummaryAvailable = true;
    expect(commands.find((command) => command.id === "thread.summarize")?.enabled(context)).toBe(true);
    context.mailbox = "drafts";
    expect(commands.find((command) => command.id === "thread.summarize")?.enabled(context)).toBe(false);
    context.mailbox = "inbox";
    context.composerActive = true;
    expect(commands.find((command) => command.id === "thread.summarize")?.enabled(context)).toBe(false);
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

  it("pages the message pane with space, reserving shift+space to page up", () => {
    const pageDown = commands.find((command) => command.id === "thread.pageDown");
    const pageUp = commands.find((command) => command.id === "thread.pageUp");
    expect(pageDown?.keys).toEqual(["Space"]);
    expect(pageUp?.keys).toEqual(["Shift+Space"]);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: " ", code: "Space" }), "Space")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: " ", code: "Space", shiftKey: true }), "Space")).toBe(false);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: " ", code: "Space", shiftKey: true }), "Shift+Space")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: " ", code: "Space" }), "Shift+Space")).toBe(false);
  });

  it("only pages the message pane when a thread is open and the composer is closed", () => {
    const pageDown = commands.find((command) => command.id === "thread.pageDown");
    expect(pageDown?.enabled({ ...noopContext(), selectedId: null })).toBe(false);
    expect(pageDown?.enabled({ ...noopContext(), selectedId: "t1", composerActive: true })).toBe(false);
    expect(pageDown?.enabled({ ...noopContext(), selectedId: "t1" })).toBe(true);
  });

  it("registers both last-action undo shortcuts", () => {
    const undo = commands.find((command) => command.id === "action.undo");
    expect(undo?.keys).toEqual(["z", "Mod+z"]);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "Z" }), "z")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "z", ctrlKey: true }), "Mod+z")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "z", metaKey: true }), "Mod+z")).toBe(true);
  });

  it("cycles split inbox tabs with Tab/Shift+Tab, only while viewing the Inbox or a split, with splits to cycle through", () => {
    const next = commands.find((command) => command.id === "mailbox.nextSplit");
    const previous = commands.find((command) => command.id === "mailbox.previousSplit");
    expect(next?.keys).toEqual(["Tab"]);
    expect(previous?.keys).toEqual(["Shift+Tab"]);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "Tab" }), "Tab")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "Tab", shiftKey: true }), "Tab")).toBe(false);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "Tab", shiftKey: true }), "Shift+Tab")).toBe(true);

    const context = { ...noopContext(), splitInboxCount: 2 };
    expect(next?.enabled(context)).toBe(true);
    expect(previous?.enabled(context)).toBe(true);
    expect(next?.enabled({ ...context, splitInboxCount: 0 })).toBe(false);
    expect(next?.enabled({ ...context, mailbox: "drafts" })).toBe(false);
    expect(next?.enabled({ ...context, mailbox: "trash" })).toBe(false);
    expect(next?.enabled({ ...context, composerActive: true })).toBe(false);
    expect(next?.enabled({ ...context, mailbox: "split" })).toBe(true);
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

  it("binds Mod+0 to show all accounts and calls showAllAccounts when run", async () => {
    const context = noopContext();
    context.showAllAccounts = vi.fn();
    const command = showAllAccountsCommand();
    expect(command.keys).toEqual(["Mod+0"]);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "0", metaKey: true }), "Mod+0")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "0", ctrlKey: true }), "Mod+0")).toBe(true);
    await command.run(context);
    expect(context.showAllAccounts).toHaveBeenCalledTimes(1);
  });

  it("does not collide with the static command registry's shortcut keys", () => {
    const staticKeys = new Set(commands.flatMap((command) => command.keys.map((key) => key.toLowerCase())));
    expect(staticKeys.has(showAllAccountsCommand().keys[0].toLowerCase())).toBe(false);
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
