import type { MessageFilterKind } from "./messageFilters";
import type { SplitInbox, TaskStatus } from "./domain";
import { adjacentTaskStatus, isActiveTaskStatus } from "./taskViews";

export type MailboxKind = "inbox" | "allMail" | "trash" | "drafts" | "outbox" | "split";
export type InteractionScope = "read" | "compose" | "search" | "modal" | "palette";
export type FocusedPane = "mail" | "tasks" | "contacts";

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
  discardDraft(): void;
  draftReplyWithAI(): void;
  undoSend(): void;
  selectNext(): void;
  selectPrevious(): void;
  selectNextTask(): void;
  selectPreviousTask(): void;
  openSelectedTask(): void;
  openTaskDetails(): void;
  completeSelectedTask(): void;
  reopenSelectedTask(): void;
  moveSelectedTask(direction: -1 | 1): void;
  selectAdjacentTaskColumn(direction: -1 | 1): void;
  toggleTaskLayout(): void;
  cycleTaskView(direction: -1 | 1): void;
  taskBoardActive: boolean;
  /** The full-width calendar week view owns Tab for week navigation. */
  calendarWeekActive: boolean;
  selectedTaskStatus: TaskStatus | null;
  selectedTaskHasThread: boolean;
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
  openMailView(): void;
  openTasksView(): void;
  openContactsView(): void;
  openCalendarView(): void;
  /** Shows the conversation's AI brief and fetches suggestions if missing. */
  getSuggestions(): void;
  /** Shows the context panel and focuses its question box. */
  openThreadChat(): void;
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
  { id: "draft.replyAll", title: "Reply All", keys: ["a"], group: "Compose", enabled: (c) => c.selectedId !== null && isThreadMailbox(c) && !c.composerActive, run: (c) => complete(c.replyAll) },
  { id: "draft.forward", title: "Forward", keys: ["f"], group: "Compose", enabled: (c) => c.selectedId !== null && isThreadMailbox(c) && !c.composerActive, run: (c) => complete(c.forward) },
  { id: "mailbox.inbox", title: "Go to Inbox", keys: ["g then i"], group: "Navigation", enabled: (c) => !c.composerActive, run: (c) => complete(c.openInbox) },
  { id: "mailbox.allMail", title: "Go to All Mail", keys: ["g then a"], group: "Navigation", enabled: (c) => !c.composerActive, run: (c) => complete(c.openAllMail) },
  { id: "mailbox.trash", title: "Go to Trash", keys: ["g then t"], group: "Navigation", enabled: (c) => !c.composerActive, run: (c) => complete(c.openTrash) },
  { id: "drafts.open", title: "Go to Drafts", keys: ["g then d"], group: "Navigation", enabled: (c) => !c.composerActive, run: (c) => complete(c.openDrafts) },
  { id: "outbox.open", title: "Open Outbox", keys: ["g then o"], group: "Navigation", enabled: (c) => !c.composerActive, run: (c) => complete(c.openOutbox) },
  {
    id: "mailbox.nextSplit",
    title: "Next Split Inbox",
    keys: ["Tab"],
    group: "Navigation",
    enabled: (c) => !c.composerActive && c.focusedPane === "mail" && !c.calendarWeekActive && (c.mailbox === "inbox" || c.mailbox === "split") && c.splitInboxCount > 0,
    run: (c) => complete(c.goToNextSplitTab),
  },
  {
    id: "mailbox.previousSplit",
    title: "Previous Split Inbox",
    keys: ["Shift+Tab"],
    group: "Navigation",
    enabled: (c) => !c.composerActive && c.focusedPane === "mail" && !c.calendarWeekActive && (c.mailbox === "inbox" || c.mailbox === "split") && c.splitInboxCount > 0,
    run: (c) => complete(c.goToPreviousSplitTab),
  },
  { id: "draft.send", title: "Send Draft", keys: ["Mod+Enter"], group: "Compose", enabled: (c) => c.composerActive, run: (c) => complete(c.sendDraft) },
  { id: "draft.sendAndMarkDone", title: "Send & Mark Done", keys: ["Mod+Shift+Enter"], group: "Compose", enabled: (c) => c.composerActive && c.canSendAndMarkDone, run: (c) => complete(c.sendAndMarkDone) },
  // The open draft in the Drafts folder is what "#" deletes there, matching
  // the composer's trash button. Typing "#" in a composer field still types.
  { id: "draft.discard", title: "Discard Draft", keys: ["#"], group: "Compose", enabled: (c) => c.composerActive && c.focusedPane === "mail" && c.mailbox === "drafts", run: (c) => complete(c.discardDraft) },
  { id: "draft.attach", title: "Attach Files", keys: [], group: "Compose", enabled: (c) => c.composerActive, run: (c) => complete(c.attachFiles) },
  { id: "draft.replyAssist", title: "Draft Reply With AI", keys: ["Mod+j"], group: "Compose", enabled: (c) => c.composerActive, run: (c) => complete(c.draftReplyWithAI) },
  { id: "send.undo", title: "Undo Send", keys: [], group: "Compose", enabled: (c) => c.canUndoSend, run: (c) => complete(c.undoSend) },
  {
    id: "thread.next",
    title: "Next Conversation",
    keys: ["j", "ArrowDown"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "mail" && !context.composerActive,
    run: (context) => complete(context.selectNext),
  },
  {
    id: "thread.previous",
    title: "Previous Conversation",
    keys: ["k", "ArrowUp"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "mail" && !context.composerActive,
    run: (context) => complete(context.selectPrevious),
  },
  {
    id: "tasks.next",
    title: "Next Task",
    keys: ["j", "ArrowDown"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "tasks" && !context.composerActive,
    run: (context) => complete(context.selectNextTask),
  },
  {
    id: "tasks.previous",
    title: "Previous Task",
    keys: ["k", "ArrowUp"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "tasks" && !context.composerActive,
    run: (context) => complete(context.selectPreviousTask),
  },
  {
    id: "tasks.openDetails",
    title: "Open Task Details",
    keys: ["Enter"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "tasks" && !context.composerActive,
    run: (context) => complete(context.openTaskDetails),
  },
  {
    id: "tasks.openSelected",
    title: "Open Task Conversation",
    keys: ["o"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "tasks" && context.selectedTaskHasThread && !context.composerActive,
    run: (context) => complete(context.openSelectedTask),
  },
  {
    id: "tasks.completeSelected",
    title: "Complete Selected Task",
    keys: ["e"],
    group: "Triage",
    enabled: (context) => context.focusedPane === "tasks" && context.selectedTaskStatus !== null && isActiveTaskStatus(context.selectedTaskStatus) && !context.composerActive,
    run: (context) => complete(context.completeSelectedTask),
  },
  {
    id: "tasks.reopenSelected",
    title: "Reopen Selected Task",
    keys: ["Shift+e"],
    group: "Triage",
    enabled: (context) => context.focusedPane === "tasks" && (context.selectedTaskStatus === "completed" || context.selectedTaskStatus === "cancelled") && !context.composerActive,
    run: (context) => complete(context.reopenSelectedTask),
  },
  {
    id: "tasks.moveForward",
    title: "Move Task to Next Column",
    keys: ["]", "Shift+ArrowRight"],
    group: "Triage",
    enabled: (context) => context.focusedPane === "tasks" && context.selectedTaskStatus !== null && adjacentTaskStatus(context.selectedTaskStatus, 1) !== null && !context.composerActive,
    run: (context) => complete(() => context.moveSelectedTask(1)),
  },
  {
    id: "tasks.moveBack",
    title: "Move Task to Previous Column",
    keys: ["[", "Shift+ArrowLeft"],
    group: "Triage",
    enabled: (context) => context.focusedPane === "tasks" && context.selectedTaskStatus !== null && adjacentTaskStatus(context.selectedTaskStatus, -1) !== null && !context.composerActive,
    run: (context) => complete(() => context.moveSelectedTask(-1)),
  },
  {
    id: "tasks.nextColumn",
    title: "Next Board Column",
    keys: ["ArrowRight"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "tasks" && context.taskBoardActive && !context.composerActive,
    run: (context) => complete(() => context.selectAdjacentTaskColumn(1)),
  },
  {
    id: "tasks.previousColumn",
    title: "Previous Board Column",
    keys: ["ArrowLeft"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "tasks" && context.taskBoardActive && !context.composerActive,
    run: (context) => complete(() => context.selectAdjacentTaskColumn(-1)),
  },
  {
    id: "tasks.toggleLayout",
    title: "Toggle Task Board",
    keys: ["v"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "tasks" && !context.composerActive,
    run: (context) => complete(context.toggleTaskLayout),
  },
  {
    id: "tasks.nextView",
    title: "Next Task View",
    keys: ["Tab"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "tasks" && !context.composerActive,
    run: (context) => complete(() => context.cycleTaskView(1)),
  },
  {
    id: "tasks.previousView",
    title: "Previous Task View",
    keys: ["Shift+Tab"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "tasks" && !context.composerActive,
    run: (context) => complete(() => context.cycleTaskView(-1)),
  },
  {
    id: "message.next",
    title: "Next Message",
    keys: ["n", "ArrowRight"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "mail" && context.canNavigateMessages && !context.composerActive,
    run: (context) => complete(context.selectNextMessage),
  },
  {
    id: "message.previous",
    title: "Previous Message",
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
    title: "Mark Not Done",
    keys: ["Shift+e"],
    group: "Triage",
    enabled: (context) =>
      context.selectedId !== null && context.selectedArchived && isThreadMailbox(context) && !context.composerActive,
    run: (context) => context.markNotDoneSelected(),
    undo: undoResult,
  },
  {
    id: "thread.check",
    title: "Select for Batch Actions",
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
    title: "Mark Spam",
    keys: ["!"],
    group: "Triage",
    enabled: (context) => context.selectedId !== null && isThreadMailbox(context) && !context.composerActive,
    run: (context) => context.markSpamSelected(),
    undo: undoResult,
  },
  {
    id: "thread.read",
    title: "Toggle Read",
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
    title: "Toggle Star",
    keys: ["s"],
    group: "Triage",
    enabled: (context) => context.selectedId !== null && isThreadMailbox(context) && !context.composerActive,
    run: (context) => context.toggleStarSelected(),
    undo: undoResult,
  },
  {
    id: "thread.toggleOlderMessages",
    title: "Expand Message",
    keys: ["o"],
    group: "Triage",
    enabled: (context) => context.selectedId !== null && isThreadMailbox(context) && !context.composerActive,
    run: (context) => complete(context.toggleOlderMessagesExpanded),
  },
  {
    id: "thread.pageDown",
    title: "Scroll Message Down",
    keys: ["Space"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "mail" && context.selectedId !== null && !context.composerActive,
    run: (context) => complete(context.pageMessageDown),
  },
  {
    id: "thread.pageUp",
    title: "Scroll Message Up",
    keys: ["Shift+Space"],
    group: "Navigation",
    enabled: (context) => context.focusedPane === "mail" && context.selectedId !== null && !context.composerActive,
    run: (context) => complete(context.pageMessageUp),
  },
  {
    id: "thread.summarize",
    title: "Summarize With AI",
    keys: ["i"],
    group: "Triage",
    enabled: (context) =>
      context.selectedId !== null && isThreadMailbox(context) && !context.composerActive && context.aiSummaryAvailable,
    run: (context) => context.summarizeSelected(),
  },
  {
    id: "labels.open",
    title: "Manage Labels",
    keys: ["l"],
    group: "Triage",
    enabled: (context) => context.selectedId !== null && isThreadMailbox(context) && !context.composerActive,
    run: (context) => complete(context.openLabels),
  },
  {
    id: "search.focus",
    title: "Search Mail",
    keys: ["/"],
    group: "Application",
    enabled: () => true,
    run: (context) => complete(context.focusSearch),
  },
  {
    id: "palette.open",
    title: "Command Palette",
    keys: ["Mod+k"],
    group: "Application",
    enabled: () => true,
    run: (context) => complete(context.openPalette),
  },
  {
    id: "shortcuts.open",
    title: "Keyboard Shortcuts",
    keys: ["?"],
    group: "Application",
    enabled: () => true,
    run: (context) => complete(context.openShortcutHelp),
  },
  {
    id: "calendar.today",
    title: "Toggle Today’s Schedule",
    keys: ["T"],
    group: "Application",
    enabled: (context) => !context.composerActive,
    run: (context) => complete(context.openToday),
  },
  {
    id: "view.mail",
    title: "Go to Mail View",
    keys: ["1"],
    group: "Navigation",
    enabled: (context) => !context.composerActive,
    run: (context) => complete(context.openMailView),
  },
  {
    id: "view.calendar",
    title: "Go to Calendar View",
    keys: ["2"],
    group: "Navigation",
    enabled: (context) => !context.composerActive,
    run: (context) => complete(context.openCalendarView),
  },
  {
    id: "view.tasks",
    title: "Go to Task View",
    keys: ["3"],
    group: "Navigation",
    enabled: (context) => !context.composerActive,
    run: (context) => complete(context.openTasksView),
  },
  {
    id: "view.contacts",
    title: "Go to Contacts Address Book",
    keys: ["4"],
    group: "Navigation",
    enabled: (context) => !context.composerActive,
    run: (context) => complete(context.openContactsView),
  },
  {
    id: "tasks.open",
    title: "Open Tasks",
    keys: ["g then k"],
    group: "Navigation",
    enabled: (context) => !context.composerActive,
    run: (context) => complete(context.openTasks),
  },
  {
    id: "tasks.new",
    title: "Add Task From Conversation",
    keys: ["d"],
    group: "Application",
    enabled: (context) => (context.focusedPane === "tasks" || context.selectedId !== null) && !context.composerActive,
    run: (context) => complete(context.newTask),
  },
  {
    id: "actions.open",
    title: "Get Suggestions",
    keys: ["Shift+a", "Mod+Shift+j"],
    group: "Application",
    enabled: (context) => context.selectedId !== null && !context.composerActive,
    run: (context) => complete(context.getSuggestions),
  },
  {
    id: "chat.open",
    title: "Ask About This Conversation",
    keys: ["q", "Mod+j"],
    group: "Application",
    enabled: (context) => context.selectedId !== null && !context.composerActive && context.focusedPane === "mail" && isThreadMailbox(context),
    run: (context) => complete(context.openThreadChat),
  },
  {
    id: "mail.refresh",
    title: "Refresh Mail",
    keys: [],
    group: "Application",
    enabled: () => true,
    run: (context) => complete(context.refresh),
  },
  {
    id: "filter.unread",
    title: "Toggle Unread Filter",
    keys: ["Shift+u"],
    group: "Application",
    enabled: (context) => isThreadMailbox(context) && !context.composerActive,
    run: (context) => complete(() => context.toggleMessageFilter("unread")),
  },
  {
    id: "filter.starred",
    title: "Toggle Starred Filter",
    keys: ["Shift+s"],
    group: "Application",
    enabled: (context) => isThreadMailbox(context) && !context.composerActive,
    run: (context) => complete(() => context.toggleMessageFilter("starred")),
  },
  {
    id: "filter.important",
    title: "Toggle Important Filter",
    keys: ["Shift+i"],
    group: "Application",
    enabled: (context) => isThreadMailbox(context) && !context.composerActive,
    run: (context) => complete(() => context.toggleMessageFilter("important")),
  },
  {
    id: "filter.noReply",
    title: "Toggle No Reply Filter",
    keys: ["Shift+r"],
    group: "Application",
    enabled: (context) => isThreadMailbox(context) && !context.composerActive,
    run: (context) => complete(() => context.toggleMessageFilter("noReply")),
  },
  {
    id: "settings.open",
    title: "Open Settings",
    keys: ["Mod+,"],
    group: "Application",
    enabled: () => true,
    run: (context) => complete(context.openSettings),
  },
  {
    id: "font.increase",
    title: "Increase Font Size",
    keys: ["Mod+=", "Mod++"],
    group: "Application",
    enabled: () => true,
    run: (context) => complete(context.increaseFontSize),
  },
  {
    id: "font.decrease",
    title: "Decrease Font Size",
    keys: ["Mod+-"],
    group: "Application",
    enabled: () => true,
    run: (context) => complete(context.decreaseFontSize),
  },
  {
    id: "action.undo",
    title: "Undo Last Action",
    keys: ["z", "Mod+z"],
    group: "Application",
    enabled: (context) => context.canUndoAction && !context.composerActive,
    run: (context) => complete(context.undoLastAction),
  },
];

export function labelCommand(labelId: string, labelName: string, value: boolean): Command {
  return {
    id: value ? "thread.label.add" : "thread.label.remove",
    title: `${value ? "Add" : "Remove"} Label ${labelName}`,
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
    title: "Show All Accounts",
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
