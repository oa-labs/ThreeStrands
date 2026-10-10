import { describe, expect, it, vi } from "vitest";
import {
  accountCommand,
  commandMatchesFilter,
  commands,
  isEditableTarget,
  matchesShortcut,
  shortcutSteps,
  showAllAccountsCommand,
  type CommandContext,
} from "./commands";

function noopContext(): CommandContext {
  return {
    interactionScope: "read",
    focusedPane: "mail",
    mailbox: "inbox",
    selectedId: null,
    selectedArchived: false,
    selectedTrashed: false,
    canUnsubscribe: false,
    canNavigateMessages: false,
    canSendAndMarkDone: false,
    composerActive: false,
    canUndoSend: false,
    canGoBack: false,
    goBack: () => {},
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
    discardDraft: () => {},
    draftReplyWithAI: () => {},
    toggleContextPanelFocus: () => {},
    undoSend: () => {},
    selectNext: () => {},
    selectPrevious: () => {},
    selectNextTask: () => {},
    selectPreviousTask: () => {},
    openSelectedTask: () => {},
    openTaskDetails: () => {},
    focusGoals: () => {},
    linkTaskToGoal: () => {},
    completeSelectedTask: () => {},
    reopenSelectedTask: () => {},
    selectedTaskStatus: null,
    moveSelectedTask: () => {},
    selectAdjacentTaskColumn: () => {},
    toggleTaskLayout: () => {},
    cycleTaskView: () => {},
    cycleContactsView: () => {},
    taskBoardActive: false,
    calendarWeekActive: false,
    selectedTaskHasThread: false,
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
    openMailView: () => {},
    openTasksView: () => {},
    openContactsView: () => {},
    openKeepInTouchView: () => {},
    openContactGroupsView: () => {},
    openCalendarView: () => {},
    getSuggestions: () => {},
    openThreadChat: () => {},
    newTask: () => {},
    increaseFontSize: () => {},
    decreaseFontSize: () => {},
    canUndoAction: false,
    canUndoArchive: false,
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

  it("keeps shortcut collisions limited to commands in mutually exclusive focused panes", () => {
    const owners = new Map<string, string[]>();
    for (const command of commands) {
      for (const key of command.keys) {
        const normalized = key.toLowerCase();
        owners.set(normalized, [...(owners.get(normalized) ?? []), command.id]);
      }
    }
    expect(Object.fromEntries([...owners].filter(([, ids]) => ids.length > 1))).toEqual({
      j: ["thread.next", "tasks.next"],
      arrowdown: ["thread.next", "tasks.next"],
      k: ["thread.previous", "tasks.previous"],
      arrowup: ["thread.previous", "tasks.previous"],
      e: ["tasks.completeSelected", "thread.archive"],
      "shift+e": ["tasks.reopenSelected", "thread.unarchive"],
      o: ["tasks.openSelected", "thread.toggleOlderMessages"],
      "#": ["draft.discard", "thread.trash"],
      arrowright: ["tasks.nextColumn", "message.next"],
      arrowleft: ["tasks.previousColumn", "message.previous"],
      "mod+j": ["draft.replyAssist", "chat.open"],
      tab: ["mailbox.nextSplit", "tasks.nextView", "contacts.nextView"],
      "shift+tab": ["mailbox.previousSplit", "contacts.previousView", "tasks.previousView"],
    });

    for (const focusedPane of ["mail", "tasks", "contacts"] as const) {
      for (const selectedTaskStatus of ["open", "in_progress", "completed", "cancelled"] as const) {
        for (const [selectedArchived, taskBoardActive] of [[false, false], [true, false], [false, true], [true, true]] as const) {
          for (const mailbox of ["inbox", "drafts"] as const) {
            for (const composerActive of [false, true]) {
              for (const canNavigateMessages of [false, true]) {
                for (const selectedTaskHasThread of [false, true]) {
                  const context = { ...noopContext(), focusedPane, selectedId: "thread-1", selectedArchived, taskBoardActive, selectedTaskStatus, mailbox, composerActive, canNavigateMessages, selectedTaskHasThread };
                  for (const ids of [...owners.values()].filter((candidateIds) => candidateIds.length > 1)) {
                    expect(ids.filter((id) => commands.find((command) => command.id === id)?.enabled(context)).length).toBeLessThanOrEqual(1);
                  }
                }
              }
            }
          }
        }
      }
    }
  });

  it("goes back with Escape only after a jump and never while composing", () => {
    const back = commands.find((command) => command.id === "navigation.back")!;
    expect(back.keys).toEqual(["Escape"]);
    expect(back.group).toBe("Navigation");
    expect(back.enabled(noopContext())).toBe(false);
    expect(back.enabled({ ...noopContext(), canGoBack: true })).toBe(true);
    expect(back.enabled({ ...noopContext(), canGoBack: true, composerActive: true })).toBe(false);
  });

  it("routes task navigation only while the Tasks pane is focused", () => {
    const taskContext = { ...noopContext(), focusedPane: "tasks" as const };
    expect(commands.find((command) => command.id === "tasks.next")?.enabled(taskContext)).toBe(true);
    expect(commands.find((command) => command.id === "thread.next")?.enabled(taskContext)).toBe(false);
    expect(commands.find((command) => command.id === "tasks.openDetails")?.keys).toEqual(["Enter"]);
    expect(commands.find((command) => command.id === "tasks.focusGoals")?.keys).toEqual(["g then g"]);
    expect(commands.find((command) => command.id === "tasks.focusGoals")?.enabled(taskContext)).toBe(true);
    expect(commands.find((command) => command.id === "tasks.focusGoals")?.enabled(noopContext())).toBe(false);
    expect(commands.find((command) => command.id === "tasks.linkGoal")?.keys).toEqual(["Shift+g"]);
    expect(commands.find((command) => command.id === "tasks.linkGoal")?.enabled({ ...taskContext, selectedTaskStatus: "open" })).toBe(true);
    expect(commands.find((command) => command.id === "tasks.linkGoal")?.enabled(taskContext)).toBe(false);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "G", shiftKey: true }), "Shift+g")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "G", shiftKey: true }), "g")).toBe(false);
    expect(commands.find((command) => command.id === "tasks.completeSelected")?.keys).toEqual(["e"]);
    expect(commands.find((command) => command.id === "tasks.reopenSelected")?.keys).toEqual(["Shift+e"]);
    expect(commands.find((command) => command.id === "tasks.reopenSelected")?.enabled({ ...taskContext, selectedTaskStatus: "cancelled" })).toBe(true);
  });

  it("moves the selected task between board columns and only navigates columns on the board", () => {
    const enabled = (id: string, context: ReturnType<typeof noopContext>) => commands.find((command) => command.id === id)?.enabled(context);
    const taskContext = { ...noopContext(), focusedPane: "tasks" as const };
    expect(commands.find((command) => command.id === "tasks.moveForward")?.keys).toEqual(["]", "Shift+ArrowRight"]);
    expect(commands.find((command) => command.id === "tasks.moveBack")?.keys).toEqual(["[", "Shift+ArrowLeft"]);
    expect(enabled("tasks.moveForward", { ...taskContext, selectedTaskStatus: "open" })).toBe(true);
    expect(enabled("tasks.moveBack", { ...taskContext, selectedTaskStatus: "open" })).toBe(false);
    expect(enabled("tasks.moveForward", { ...taskContext, selectedTaskStatus: "completed" })).toBe(false);
    expect(enabled("tasks.moveBack", { ...taskContext, selectedTaskStatus: "completed" })).toBe(true);
    expect(enabled("tasks.completeSelected", { ...taskContext, selectedTaskStatus: "in_progress" })).toBe(true);
    expect(enabled("tasks.reopenSelected", { ...taskContext, selectedTaskStatus: "in_progress" })).toBe(false);
    expect(enabled("tasks.moveForward", { ...noopContext(), selectedTaskStatus: "open" })).toBe(false);
    expect(enabled("tasks.nextColumn", taskContext)).toBe(false);
    expect(enabled("tasks.nextColumn", { ...taskContext, taskBoardActive: true })).toBe(true);
    expect(enabled("tasks.toggleLayout", taskContext)).toBe(true);

    const moveSelectedTask = vi.fn();
    void commands.find((command) => command.id === "tasks.moveBack")?.run({ ...taskContext, moveSelectedTask });
    expect(moveSelectedTask).toHaveBeenCalledWith(-1);
  });

  it("disables mail and task arrow-key navigation while the Contacts pane is focused", () => {
    const contactsContext = { ...noopContext(), focusedPane: "contacts" as const };
    expect(commands.find((command) => command.id === "thread.next")?.enabled(contactsContext)).toBe(false);
    expect(commands.find((command) => command.id === "thread.previous")?.enabled(contactsContext)).toBe(false);
    expect(commands.find((command) => command.id === "tasks.next")?.enabled(contactsContext)).toBe(false);
    expect(commands.find((command) => command.id === "tasks.previous")?.enabled(contactsContext)).toBe(false);
  });

  it("registers direct primary-view shortcuts without a shortcut for 0", () => {
    expect(commands.find((command) => command.id === "view.mail")?.keys).toEqual(["1"]);
    expect(commands.find((command) => command.id === "view.calendar")?.keys).toEqual(["2"]);
    expect(commands.find((command) => command.id === "view.tasks")?.keys).toEqual(["3"]);
    expect(commands.find((command) => command.id === "view.contacts")?.keys).toEqual(["4"]);
    expect(commands.some((command) => command.keys.includes("0"))).toBe(false);
  });

  it("routes the Contacts shortcut to the address book outside the composer", async () => {
    const command = commands.find((candidate) => candidate.id === "view.contacts");
    const openContactsView = vi.fn();
    expect(command?.enabled(noopContext())).toBe(true);
    expect(command?.enabled({ ...noopContext(), composerActive: true })).toBe(false);
    await command?.run({ ...noopContext(), openContactsView });
    expect(openContactsView).toHaveBeenCalledTimes(1);
  });

  it("opens Keep in Touch from the palette without claiming a shortcut", async () => {
    const command = commands.find((candidate) => candidate.id === "view.keepInTouch");
    const openKeepInTouchView = vi.fn();
    expect(command?.keys).toEqual([]);
    expect(command?.enabled({ ...noopContext(), composerActive: true })).toBe(false);
    await command?.run({ ...noopContext(), openKeepInTouchView });
    expect(openKeepInTouchView).toHaveBeenCalledTimes(1);
  });

  it("opens Contact Groups from the palette without claiming a shortcut", async () => {
    const command = commands.find((candidate) => candidate.id === "view.contactGroups");
    const openContactGroupsView = vi.fn();
    expect(command?.keys).toEqual([]);
    expect(command?.enabled({ ...noopContext(), composerActive: true })).toBe(false);
    await command?.run({ ...noopContext(), openContactGroupsView });
    expect(openContactGroupsView).toHaveBeenCalledTimes(1);
  });

  it("matches palette filters word by word against titles and keywords", () => {
    const command = commands.find((candidate) => candidate.id === "tasks.new")!;
    expect(commandMatchesFilter(command, "")).toBe(true);
    expect(commandMatchesFilter(command, "Add Task")).toBe(true);
    expect(commandMatchesFilter(command, "  task   NEW ")).toBe(true);
    expect(commandMatchesFilter(command, "create todo")).toBe(true);
    expect(commandMatchesFilter(command, "new widget")).toBe(false);
    const untagged = commands.find((candidate) => candidate.id === "shortcuts.open")!;
    expect(untagged.keywords).toBeUndefined();
    expect(commandMatchesFilter(untagged, "keyboard shortcuts")).toBe(true);
    expect(commandMatchesFilter(untagged, "new")).toBe(false);
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
    expect(commands.find((command) => command.id === "outbox.open")?.keys).toEqual(["g then o"]);
    expect(commands.find((command) => command.id === "outbox.open")?.enabled({ ...noopContext(), composerActive: true })).toBe(false);
    expect(commands.find((command) => command.id === "labels.open")?.keys).toEqual(["l"]);
  });

  it("uses d for adding a task from the conversation", async () => {
    const openTasks = commands.find((command) => command.id === "tasks.open");
    const newTask = commands.find((command) => command.id === "tasks.new");
    expect(openTasks?.keys).toEqual(["g then k"]);
    expect(newTask?.keys).toEqual(["d"]);
    const start = vi.fn();
    await newTask?.run({ ...noopContext(), selectedId: "thread-1", newTask: start });
    expect(start).toHaveBeenCalledTimes(1);
    expect(newTask?.enabled({ ...noopContext(), focusedPane: "tasks", selectedId: null })).toBe(true);
  });

  it("opens thread chat with q or Mod+J in read mode while Mod+J keeps drafting in the composer", () => {
    const chat = commands.find((command) => command.id === "chat.open");
    expect(chat?.keys).toEqual(["q", "Mod+j"]);
    const reading = { ...noopContext(), selectedId: "thread-1", focusedPane: "mail" as const, mailbox: "inbox" as const };
    expect(chat?.enabled(reading)).toBe(true);
    expect(chat?.enabled({ ...reading, composerActive: true })).toBe(false);
    expect(chat?.enabled({ ...reading, selectedId: null })).toBe(false);
    expect(chat?.enabled({ ...reading, focusedPane: "tasks" })).toBe(false);
    const draftAssist = commands.find((command) => command.id === "draft.replyAssist");
    expect(draftAssist?.keys).toContain("Mod+j");
    expect(draftAssist?.enabled({ ...reading, composerActive: true })).toBe(true);
    expect(draftAssist?.enabled(reading)).toBe(false);
    expect(commands.filter((command) => command.keys.includes("q"))).toHaveLength(1);
  });

  it("binds reply to r and reply all to a, with Shift+A for Get Suggestions", () => {
    expect(commands.find((command) => command.id === "draft.reply")?.keys).toEqual(["r"]);
    expect(commands.find((command) => command.id === "draft.replyAll")?.keys).toEqual(["a"]);
    expect(commands.find((command) => command.id === "actions.open")?.keys).toContain("Shift+a");
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

  it("moves between the draft and the context panel on F6 or Mod+Shift+P while the composer is open", async () => {
    const command = commands.find((candidate) => candidate.id === "draft.contextPanel");
    expect(command?.keys).toEqual(["F6", "Mod+Shift+p"]);
    expect(command?.enabled({ ...noopContext(), composerActive: true })).toBe(true);
    expect(command?.enabled(noopContext())).toBe(false);
    const toggleContextPanelFocus = vi.fn();
    await command?.run({ ...noopContext(), composerActive: true, toggleContextPanelFocus });
    expect(toggleContextPanelFocus).toHaveBeenCalledTimes(1);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "F6" }), "F6")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "P", metaKey: true, shiftKey: true }), "Mod+Shift+p")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "P", ctrlKey: true, shiftKey: true }), "Mod+Shift+p")).toBe(true);
    expect(matchesShortcut(new KeyboardEvent("keydown", { key: "p", metaKey: true }), "Mod+Shift+p")).toBe(false);
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

  it("discards the open draft on # only in the Drafts folder", async () => {
    const command = commands.find((candidate) => candidate.id === "draft.discard");
    expect(command?.keys).toEqual(["#"]);
    expect(command?.enabled({ ...noopContext(), mailbox: "drafts" })).toBe(false);
    expect(command?.enabled({ ...noopContext(), composerActive: true })).toBe(false);
    expect(command?.enabled({ ...noopContext(), composerActive: true, mailbox: "drafts", focusedPane: "tasks" })).toBe(false);
    expect(command?.enabled({ ...noopContext(), composerActive: true, mailbox: "drafts" })).toBe(true);
    const discardDraft = vi.fn();
    await command?.run({ ...noopContext(), composerActive: true, mailbox: "drafts", discardDraft });
    expect(discardDraft).toHaveBeenCalledTimes(1);
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

  it("offers Shift+E for the latest archive even without a selection, only in a mail reading context", async () => {
    const command = commands.find((candidate) => candidate.id === "thread.unarchive")!;
    const undoLastAction = vi.fn();
    const context = { ...noopContext(), canUndoArchive: true, undoLastAction };
    expect(command.enabled(context)).toBe(true);
    await command.run(context);
    expect(undoLastAction).toHaveBeenCalledOnce();
    expect(command.enabled({ ...context, canUndoArchive: false })).toBe(false);
    expect(command.enabled({ ...context, focusedPane: "tasks" })).toBe(false);
    expect(command.enabled({ ...context, focusedPane: "contacts" })).toBe(false);
    expect(command.enabled({ ...context, composerActive: true })).toBe(false);
    expect(command.enabled({ ...context, mailbox: "drafts" })).toBe(false);
    expect(command.enabled({ ...context, mailbox: "outbox" })).toBe(false);
  });

  it("preserves Shift+E for the selected archived conversation", async () => {
    const command = commands.find((candidate) => candidate.id === "thread.unarchive")!;
    const undoLastAction = vi.fn();
    const markNotDoneSelected = vi.fn(async () => ({}));
    await command.run({ ...noopContext(), selectedId: "archived-thread", selectedArchived: true, canUndoArchive: true, undoLastAction, markNotDoneSelected });
    expect(markNotDoneSelected).toHaveBeenCalledOnce();
    expect(undoLastAction).not.toHaveBeenCalled();
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
    expect(next?.enabled({ ...context, focusedPane: "tasks" })).toBe(false);
    expect(previous?.enabled({ ...context, focusedPane: "tasks" })).toBe(false);
    // The address book leaves Tab alone, and the calendar week view uses it for week navigation.
    expect(next?.enabled({ ...context, focusedPane: "contacts" })).toBe(false);
    expect(previous?.enabled({ ...context, focusedPane: "contacts" })).toBe(false);
    expect(next?.enabled({ ...context, calendarWeekActive: true })).toBe(false);
    expect(previous?.enabled({ ...context, calendarWeekActive: true })).toBe(false);
  });

  it("cycles task views with Tab/Shift+Tab only while the Tasks pane is focused", () => {
    const next = commands.find((command) => command.id === "tasks.nextView");
    const previous = commands.find((command) => command.id === "tasks.previousView");
    expect(next?.keys).toEqual(["Tab"]);
    expect(previous?.keys).toEqual(["Shift+Tab"]);

    const taskContext = { ...noopContext(), focusedPane: "tasks" as const, splitInboxCount: 2 };
    expect(next?.enabled(taskContext)).toBe(true);
    expect(previous?.enabled(taskContext)).toBe(true);
    expect(next?.enabled({ ...taskContext, composerActive: true })).toBe(false);
    expect(next?.enabled({ ...taskContext, focusedPane: "mail" })).toBe(false);

    const cycleTaskView = vi.fn();
    void next?.run({ ...taskContext, cycleTaskView });
    void previous?.run({ ...taskContext, cycleTaskView });
    expect(cycleTaskView.mock.calls).toEqual([[1], [-1]]);
  });

  it("alternates contact views with Tab/Shift+Tab only while the Contacts pane is focused", () => {
    const next = commands.find((command) => command.id === "contacts.nextView");
    const previous = commands.find((command) => command.id === "contacts.previousView");
    expect(next?.keys).toEqual(["Tab"]);
    expect(previous?.keys).toEqual(["Shift+Tab"]);

    const contactsContext = { ...noopContext(), focusedPane: "contacts" as const, splitInboxCount: 2 };
    expect(next?.enabled(contactsContext)).toBe(true);
    expect(previous?.enabled(contactsContext)).toBe(true);
    expect(next?.enabled({ ...contactsContext, composerActive: true })).toBe(false);
    expect(next?.enabled({ ...contactsContext, focusedPane: "mail" })).toBe(false);
    expect(next?.enabled({ ...contactsContext, focusedPane: "tasks" })).toBe(false);

    const cycleContactsView = vi.fn();
    void next?.run({ ...contactsContext, cycleContactsView });
    void previous?.run({ ...contactsContext, cycleContactsView });
    expect(cycleContactsView.mock.calls).toEqual([[1], [-1]]);
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
