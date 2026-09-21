import type { MessageFilterKind } from "./messageFilters";
import type { SplitInbox } from "./domain";

export type MailboxKind = "inbox" | "allMail" | "trash" | "drafts" | "outbox" | "split";
export type InteractionScope = "read" | "compose" | "search" | "modal" | "palette";
export type FocusedPane = "mail" | "tasks";

export type CommandContext = {
  interactionScope: InteractionScope;
  focusedPane: FocusedPane;
  mailbox: MailboxKind;
  selectedId: string | null;
  selectedArchived: boolean;
  selectedTrashed: boolean;
  canUnsubscribe: boolean;
  canNavigateMessages: boolean;
  canSendAndMarkDone: boolean;
  composerActive: boolean;
  closing?: boolean;
  canUndoSend: boolean;
  compose(): void;
  reply(): void;
  replyAll(): void;
  forward(): void;
  openInbox(): void;
  openAllMail(): void;
  openTrash(): void;
  openDrafts(): void;
  openOutbox(): void;
  openSplitInbox(id: string): void;
  /** Count of user-defined split inboxes, for gating Tab/Shift+Tab cycling. */
  splitInboxCount: number;
  goToNextSplitTab(): void;
  goToPreviousSplitTab(): void;
  sendDraft(): void;
  sendAndMarkDone(): void;
  attachFiles(): void;
  draftReplyWithAI(): void;
  undoSend(): void;
  selectNext(): void;
  selectPrevious(): void;
  selectNextTask(): void;
  selectPreviousTask(): void;
  openSelectedTask(): void;
  editSelectedTask(): void;
  completeSelectedTask(): void;
  reopenSelectedTask(): void;
  selectedTaskStatus: "open" | "completed" | "cancelled" | null;
  selectNextMessage(): void;
  selectPreviousMessage(): void;
  archiveSelected(): Promise<CommandResult>;
  markNotDoneSelected(): Promise<CommandResult>;
  unsubscribeSelected(): void;
  trashSelected(): Promise<CommandResult>;
  restoreSelected(): Promise<CommandResult>;
  markSpamSelected(): Promise<CommandResult>;
  setLabelSelected(labelId: string, labelName: string, value: boolean): Promise<CommandResult>;
  toggleReadSelected(): Promise<CommandResult>;
  toggleStarSelected(): Promise<CommandResult>;
  toggleCheckedSelected(): void;
  toggleOlderMessagesExpanded(): void;
  pageMessageDown(): void;
  pageMessageUp(): void;
  aiSummaryAvailable: boolean;
  summarizeSelected(): Promise<CommandResult>;
  focusSearch(): void;
  refresh(): void;
  openLabels(): void;
  openPalette(): void;
  openShortcutHelp(): void;
  openSettings(): void;
  openToday(): void;
  openTasks(): void;
  openActions(): void;
  newTask(): void;
  increaseFontSize(): void;
  decreaseFontSize(): void;
  canUndoAction: boolean;
  undoLastAction(): void;
  showAllAccounts(): void;
  switchAccount(email: string): void;
  toggleMessageFilter(kind: MessageFilterKind): void;
};

export type CommandResult = {
  message?: string;
  undoAction?: () => Promise<void>;
};

export type Command = {
  id: string;
  title: string;
  keys: string[];
  group: "Navigation" | "Triage" | "Application" | "Compose";
  enabled(context: CommandContext): boolean;
  run(context: CommandContext): Promise<CommandResult>;
  undo?: (result: CommandResult) => Promise<void>;
};

const complete = async (action: () => void): Promise<CommandResult> => {
  action();
  return {};
};

export const undoResult = async (result: CommandResult): Promise<void> => {
  await result.undoAction?.();
};

/** Triage/reply commands only make sense against a real thread selection. */
const isThreadMailbox = (context: CommandContext): boolean =>
  context.focusedPane === "mail" && context.mailbox !== "drafts" && context.mailbox !== "outbox";

export const commands: Command[] = [
  { id: "draft.new", title: "New Message", keys: ["c"], group: "Compose", enabled: () => true, run: (c) => complete(c.compose) },
  { id: "draft.reply", title: "Reply", keys: ["r"], group: "Compose", enabled: (c) => c.selectedId !== null && isThreadMailbox(c) && !c.composerActive, run: (c) => complete(c.reply) },
  { id: "draft.replyAll", title: "Reply all", keys: ["a"], group: "Compose", enabled: (c) => c.selectedId !== null && isThreadMailbox(c) && !c.composerActive, run: (c) => complete(c.replyAll) },
  { id: "draft.forward", title: "Forward", keys: ["f"], group: "Compose", enabled: (c) => c.selectedId !== null && isThreadMailbox(c) && !c.composerActive, run: (c) => complete(c.forward) },
  { id: "mailbox.inbox", title: "Go to Inbox", keys: ["g then i"], group: "Navigation", enabled: (c) => !c.composerActive, run: (c) => complete(c.openInbox) },
  { id: "mailbox.allMail", title: "Go to All Mail", keys: ["g then a"], group: "Navigation", enabled: (c) => !c.composerActive, run: (c) => complete(c.openAllMail) },
  { id: "mailbox.trash", title: "Go to Trash", keys: ["g then t"], group: "Navigation", enabled: (c) => !c.composerActive, run: (c) => complete(c.openTrash) },
  { id: "drafts.open", title: "Go to Drafts", keys: ["g then d"], group: "Navigation", enabled: (c) => !c.composerActive, run: (c) => complete(c.openDrafts) },
  {
    id: "mailbox.nextSplit",
    title: "Next split inbox",
    keys: ["Tab"],
    group: "Navigation",
    enabled: (c) => !c.composerActive && (c.mailbox === "inbox" || c.mailbox === "split") && c.splitInboxCount > 0,
    run: (c) => complete(c.goToNextSplitTab),
  },
  {
    id: "mailbox.previousSplit",
    title: "Previous split inbox",
    keys: ["Shift+Tab"],
    group: "Navigation",
    enabled: (c) => !c.composerActive && (c.mailbox === "inbox" || c.mailbox === "split") && c.splitInboxCount > 0,
    run: (c) => complete(c.goToPreviousSplitTab),
  },
  { id: "outbox.open", title: "Open outbox", keys: [], group: "Compose", enabled: () => true, run: (c) => complete(c.openOutbox) },
  { id: "draft.send", title: "Send draft", keys: ["Mod+Enter"], group: "Compose", enabled: (c) => c.composerActive, run: (c) => complete(c.sendDraft) },
  { id: "draft.sendAndMarkDone", title: "Send & Mark Done", keys: ["Mod+Shift+Enter"], group: "Compose", enabled: (c) => c.composerActive && c.canSendAndMarkDone, run: (c) => complete(c.sendAndMarkDone) },
  { id: "draft.attach", title: "Attach files", keys: [], group: "Compose", enabled: (c) => c.composerActive, run: (c) => complete(c.attachFiles) },
  { id: "draft.replyAssist", title: "Draft reply with AI", keys: ["Mod+j"], group: "Compose", enabled: (c) => c.composerActive, run: (c) => complete(c.draftReplyWithAI) },
  { id: "send.undo", title: "Undo send", keys: [], group: "Compose", enabled: (c) => c.canUndoSend, run: (c) => complete(c.undoSend) },
  {
    id: "thread.next",
    title: "Next conversation",
    keys: ["j", "ArrowDown"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "mail" && !context.composerActive,
    run: (context) => complete(context.selectNext),
  },
  {
    id: "thread.previous",
    title: "Previous conversation",
    keys: ["k", "ArrowUp"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "mail" && !context.composerActive,
    run: (context) => complete(context.selectPrevious),
  },
  {
    id: "tasks.next",
    title: "Next task",
    keys: ["j", "ArrowDown"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "tasks" && !context.composerActive,
    run: (context) => complete(context.selectNextTask),
  },
  {
    id: "tasks.previous",
    title: "Previous task",
    keys: ["k", "ArrowUp"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "tasks" && !context.composerActive,
    run: (context) => complete(context.selectPreviousTask),
  },
  {
    id: "tasks.editSelected",
    title: "Edit selected task",
    keys: ["Enter"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "tasks" && !context.composerActive,
    run: (context) => complete(context.editSelectedTask),
  },
  {
    id: "tasks.openSelected",
    title: "Open task conversation",
    keys: ["o"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "tasks" && context.selectedTaskStatus !== null && !context.composerActive,
    run: (context) => complete(context.openSelectedTask),
  },
  {
    id: "tasks.completeSelected",
    title: "Complete selected task",
    keys: ["e"],
    group: "Triage",
    enabled: (context) => context.focusedPane === "tasks" && context.selectedTaskStatus === "open" && !context.composerActive,
    run: (context) => complete(context.completeSelectedTask),
  },
  {
    id: "tasks.reopenSelected",
    title: "Reopen selected task",
    keys: ["Shift+e"],
    group: "Triage",
    enabled: (context) => context.focusedPane === "tasks" && context.selectedTaskStatus === "completed" && !context.composerActive,
    run: (context) => complete(context.reopenSelectedTask),
  },
  {
    id: "message.next",
    title: "Next message",
    keys: ["n", "ArrowRight"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "mail" && context.canNavigateMessages && !context.composerActive,
    run: (context) => complete(context.selectNextMessage),
  },
  {
    id: "message.previous",
    title: "Previous message",
    keys: ["p", "ArrowLeft"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "mail" && context.canNavigateMessages && !context.composerActive,
    run: (context) => complete(context.selectPreviousMessage),
  },
  {
    id: "thread.archive",
    title: "Archive",
    keys: ["e"],
    group: "Triage",
    enabled: (context) => context.focusedPane === "mail" && context.selectedId !== null && isThreadMailbox(context) && !context.composerActive,
    run: (context) => context.archiveSelected(),
    undo: undoResult,
  },
  {
    id: "thread.unarchive",
    title: "Mark not done",
    keys: ["Shift+e"],
    group: "Triage",
    enabled: (context) =>
      context.selectedId !== null && context.selectedArchived && isThreadMailbox(context) && !context.composerActive,
    run: (context) => context.markNotDoneSelected(),
    undo: undoResult,
  },
  {
    id: "thread.check",
    title: "Select for batch actions",
    keys: ["x"],
    group: "Triage",
    enabled: (context) => context.focusedPane === "mail" && context.selectedId !== null && isThreadMailbox(context) && !context.composerActive,
    run: (context) => complete(context.toggleCheckedSelected),
  },
  {
    id: "thread.trash",
    title: "Trash",
    keys: ["#"],
    group: "Triage",
    enabled: (context) =>
      context.selectedId !== null && !context.selectedTrashed && isThreadMailbox(context) && !context.composerActive,
    run: (context) => context.trashSelected(),
    undo: undoResult,
  },
  {
    id: "thread.untrash",
    title: "Restore",
    keys: [],
    group: "Triage",
    enabled: (context) =>
      context.selectedId !== null && context.selectedTrashed && isThreadMailbox(context) && !context.composerActive,
    run: (context) => context.restoreSelected(),
    undo: undoResult,
  },
  {
    id: "thread.spam",
    title: "Mark spam",
    keys: ["!"],
    group: "Triage",
    enabled: (context) => context.selectedId !== null && isThreadMailbox(context) && !context.composerActive,
    run: (context) => context.markSpamSelected(),
    undo: undoResult,
  },
  {
    id: "thread.read",
    title: "Toggle read",
    keys: ["u"],
    group: "Triage",
    enabled: (context) => context.selectedId !== null && isThreadMailbox(context) && !context.composerActive,
    run: (context) => context.toggleReadSelected(),
    undo: undoResult,
  },
  {
    id: "thread.unsubscribe",
    title: "Unsubscribe",
    keys: ["Mod+u"],
    group: "Triage",
    enabled: (context) => context.canUnsubscribe && isThreadMailbox(context) && !context.composerActive,
    run: (context) => complete(context.unsubscribeSelected),
  },
  {
    id: "thread.star",
    title: "Toggle star",
    keys: ["s"],
    group: "Triage",
    enabled: (context) => context.selectedId !== null && isThreadMailbox(context) && !context.composerActive,
    run: (context) => context.toggleStarSelected(),
    undo: undoResult,
  },
  {
    id: "thread.toggleOlderMessages",
    title: "Expand message",
    keys: ["o"],
    group: "Triage",
    enabled: (context) => context.selectedId !== null && isThreadMailbox(context) && !context.composerActive,
    run: (context) => complete(context.toggleOlderMessagesExpanded),
  },
  {
    id: "thread.pageDown",
    title: "Scroll message down",
    keys: ["Space"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "mail" && context.selectedId !== null && !context.composerActive,
    run: (context) => complete(context.pageMessageDown),
  },
  {
    id: "thread.pageUp",
    title: "Scroll message up",
    keys: ["Shift+Space"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "mail" && context.selectedId !== null && !context.composerActive,
    run: (context) => complete(context.pageMessageUp),
  },
  {
    id: "thread.summarize",
    title: "Summarize with AI",
    keys: ["i"],
    group: "Triage",
    enabled: (context) =>
      context.selectedId !== null && isThreadMailbox(context) && !context.composerActive && context.aiSummaryAvailable,
    run: (context) => context.summarizeSelected(),
  },
  {
    id: "labels.open",
    title: "Manage labels",
    keys: ["l"],
    group: "Triage",
    enabled: (context) => context.selectedId !== null && isThreadMailbox(context) && !context.composerActive,
    run: (context) => complete(context.openLabels),
  },
  {
    id: "search.focus",
    title: "Search mail",
    keys: ["/"],
    group: "Application",
    enabled: () => true,
    run: (context) => complete(context.focusSearch),
  },
  {
    id: "palette.open",
    title: "Command palette",
    keys: ["Mod+k"],
    group: "Application",
    enabled: () => true,
    run: (context) => complete(context.openPalette),
  },
  {
    id: "shortcuts.open",
    title: "Keyboard shortcuts",
    keys: ["?"],
    group: "Application",
    enabled: () => true,
    run: (context) => complete(context.openShortcutHelp),
  },
  {
    id: "calendar.today",
    title: "Toggle today’s schedule",
    keys: ["T"],
    group: "Application",
    enabled: (context) => !context.composerActive,
    run: (context) => complete(context.openToday),
  },
  {
    id: "tasks.open",
    title: "Open tasks",
    keys: ["d", "g then k"],
    group: "Navigation",
    enabled: (context) => !context.composerActive,
    run: (context) => complete(context.openTasks),
  },
  {
    id: "tasks.new",
    title: "Add task from conversation",
    keys: ["Mod+d"],
    group: "Application",
    enabled: (context) => context.selectedId !== null && !context.composerActive,
    run: (context) => complete(context.newTask),
  },
  {
    id: "actions.open",
    title: "Open conversation actions",
    keys: ["Shift+a", "Mod+Shift+j"],
    group: "Application",
    enabled: (context) => context.selectedId !== null && !context.composerActive,
    run: (context) => complete(context.openActions),
  },
  {
    id: "mail.refresh",
    title: "Refresh mail",
    keys: [],
    group: "Application",
    enabled: () => true,
    run: (context) => complete(context.refresh),
  },
  {
    id: "filter.unread",
    title: "Toggle Unread filter",
    keys: ["Shift+u"],
    group: "Application",
    enabled: (context) => isThreadMailbox(context) && !context.composerActive,
    run: (context) => complete(() => context.toggleMessageFilter("unread")),
  },
  {
    id: "filter.starred",
    title: "Toggle Starred filter",
    keys: ["Shift+s"],
    group: "Application",
    enabled: (context) => isThreadMailbox(context) && !context.composerActive,
    run: (context) => complete(() => context.toggleMessageFilter("starred")),
  },
  {
    id: "filter.important",
    title: "Toggle Important filter",
    keys: ["Shift+i"],
    group: "Application",
    enabled: (context) => isThreadMailbox(context) && !context.composerActive,
    run: (context) => complete(() => context.toggleMessageFilter("important")),
  },
  {
    id: "filter.noReply",
    title: "Toggle No Reply filter",
    keys: ["Shift+r"],
    group: "Application",
    enabled: (context) => isThreadMailbox(context) && !context.composerActive,
    run: (context) => complete(() => context.toggleMessageFilter("noReply")),
  },
  {
    id: "settings.open",
    title: "Open settings",
    keys: ["Mod+,"],
    group: "Application",
    enabled: () => true,
    run: (context) => complete(context.openSettings),
  },
  {
    id: "font.increase",
    title: "Increase font size",
    keys: ["Mod+=", "Mod++"],
    group: "Application",
    enabled: () => true,
    run: (context) => complete(context.increaseFontSize),
  },
  {
    id: "font.decrease",
    title: "Decrease font size",
    keys: ["Mod+-"],
    group: "Application",
    enabled: () => true,
    run: (context) => complete(context.decreaseFontSize),
  },
  {
    id: "action.undo",
    title: "Undo last action",
    keys: ["z", "Mod+z"],
    group: "Application",
    enabled: (context) => context.canUndoAction && !context.composerActive,
    run: (context) => complete(context.undoLastAction),
  },
];

export function labelCommand(labelId: string, labelName: string, value: boolean): Command {
  return {
    id: value ? "thread.label.add" : "thread.label.remove",
    title: `${value ? "Add" : "Remove"} label ${labelName}`,
    keys: [],
    group: "Triage",
    enabled: (context) => context.selectedId !== null && isThreadMailbox(context) && !context.composerActive,
    run: (context) => context.setLabelSelected(labelId, labelName, value),
    undo: undoResult,
  };
}

/** One entry per connected account, `Mod+1`..`Mod+9` bound by sort order. */
export function accountCommand(email: string, index: number): Command {
  return {
    id: `account.switch.${email}`,
    title: `Switch to ${email}`,
    keys: index < 9 ? [`Mod+${index + 1}`] : [],
    group: "Application",
    enabled: () => true,
    run: (context) => complete(() => context.switchAccount(email)),
  };
}

/** One entry per user-defined split inbox, reachable from the command palette alongside the sidebar. */
export function splitInboxCommand(splitInbox: SplitInbox): Command {
  return {
    id: `mailbox.split.${splitInbox.id}`,
    title: `Go to ${splitInbox.name}`,
    keys: [],
    group: "Navigation",
    enabled: (context) => !context.composerActive,
    run: (context) => complete(() => context.openSplitInbox(splitInbox.id)),
  };
}

export function showAllAccountsCommand(): Command {
  return {
    id: "account.showAll",
    title: "Show all accounts",
    keys: ["Mod+0"],
    group: "Application",
    enabled: () => true,
    run: (context) => complete(context.showAllAccounts),
  };
}

export function matchesShortcut(event: KeyboardEvent, key: string): boolean {
  let base = key;
  const expectsMod = base.startsWith("Mod+");
  if (expectsMod) base = base.slice(4);
  const expectsShift = base.startsWith("Shift+");
  if (expectsShift) base = base.slice(6);
  const shiftedSymbolCode = base === "#"
    ? "Digit3"
    : base === "!"
      ? "Digit1"
      : null;
  const implicitSymbolShift = event.shiftKey && (
    (base === "+" && event.key === "+") ||
    (base === "=" && event.key === "=") ||
    (shiftedSymbolCode !== null && (event.key === base || event.code === shiftedSymbolCode))
  );
  const questionMark = base === "?" && (
    event.key === "?" || (event.key === "/" && event.shiftKey)
  );
  const spaceKey = base === "Space" && event.code === "Space";
  const shiftedDigit = expectsShift && /^\d$/.test(base) && event.code === `Digit${base}`;
  const shiftMatches = expectsShift
    ? event.shiftKey
    : base === "?" || implicitSymbolShift || !event.shiftKey;
  const shiftedSymbol = shiftedSymbolCode !== null && (
    event.key === base || (event.shiftKey && event.code === shiftedSymbolCode)
  );
  const baseMatches = questionMark || spaceKey || shiftedDigit || shiftedSymbol || event.key.toLocaleLowerCase() === base.toLocaleLowerCase();
  return baseMatches
    && shiftMatches
    && (event.ctrlKey || event.metaKey) === expectsMod
    && !event.altKey;
}

export function shortcutSteps(key: string): string[] {
  return key.split(/\s+then\s+/i);
}

export function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return (
    target.isContentEditable ||
    target.tagName === "INPUT" ||
    target.tagName === "TEXTAREA" ||
    target.tagName === "SELECT"
  );
}
