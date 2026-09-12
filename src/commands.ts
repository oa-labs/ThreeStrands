export type CommandContext = {
  selectedId: string | null;
  composerActive: boolean;
  closing?: boolean;
  canUndoSend: boolean;
  compose(): void;
  reply(): void;
  replyAll(): void;
  forward(): void;
  openDrafts(): void;
  openOutbox(): void;
  sendDraft(): void;
  attachFiles(): void;
  undoSend(): void;
  selectNext(): void;
  selectPrevious(): void;
  archiveSelected(): void;
  toggleReadSelected(): void;
  toggleStarSelected(): void;
  focusSearch(): void;
  refresh(): void;
  openDiagnostics(): void;
  openLabels(): void;
};

export type Command = {
  id: string;
  title: string;
  keys: string[];
  group: "Navigation" | "Triage" | "Application" | "Compose";
  enabled(context: CommandContext): boolean;
  run(context: CommandContext): void;
};

export const commands: Command[] = [
  { id: "draft.new", title: "New message", keys: ["c"], group: "Compose", enabled: () => true, run: (c) => c.compose() },
  { id: "draft.reply", title: "Reply", keys: ["r"], group: "Compose", enabled: (c) => c.selectedId !== null && !c.composerActive, run: (c) => c.reply() },
  { id: "draft.replyAll", title: "Reply all", keys: ["a"], group: "Compose", enabled: (c) => c.selectedId !== null && !c.composerActive, run: (c) => c.replyAll() },
  { id: "draft.forward", title: "Forward", keys: ["f"], group: "Compose", enabled: (c) => c.selectedId !== null && !c.composerActive, run: (c) => c.forward() },
  { id: "drafts.open", title: "Open drafts", keys: [], group: "Compose", enabled: () => true, run: (c) => c.openDrafts() },
  { id: "outbox.open", title: "Open outbox", keys: [], group: "Compose", enabled: () => true, run: (c) => c.openOutbox() },
  { id: "draft.send", title: "Send draft", keys: ["Mod+Enter"], group: "Compose", enabled: (c) => c.composerActive, run: (c) => c.sendDraft() },
  { id: "draft.attach", title: "Attach files", keys: [], group: "Compose", enabled: (c) => c.composerActive, run: (c) => c.attachFiles() },
  { id: "send.undo", title: "Undo send", keys: [], group: "Compose", enabled: (c) => c.canUndoSend, run: (c) => c.undoSend() },
  {
    id: "thread.next",
    title: "Next conversation",
    keys: ["j", "ArrowDown"],
    group: "Navigation",
    enabled: (context) => !context.composerActive,
    run: (context) => context.selectNext(),
  },
  {
    id: "thread.previous",
    title: "Previous conversation",
    keys: ["k", "ArrowUp"],
    group: "Navigation",
    enabled: (context) => !context.composerActive,
    run: (context) => context.selectPrevious(),
  },
  {
    id: "thread.archive",
    title: "Archive",
    keys: ["e"],
    group: "Triage",
    enabled: (context) => context.selectedId !== null && !context.composerActive,
    run: (context) => context.archiveSelected(),
  },
  {
    id: "thread.read",
    title: "Toggle read",
    keys: ["u"],
    group: "Triage",
    enabled: (context) => context.selectedId !== null && !context.composerActive,
    run: (context) => context.toggleReadSelected(),
  },
  {
    id: "thread.star",
    title: "Toggle star",
    keys: ["s"],
    group: "Triage",
    enabled: (context) => context.selectedId !== null && !context.composerActive,
    run: (context) => context.toggleStarSelected(),
  },
  {
    id: "labels.open",
    title: "Manage labels",
    keys: ["v"],
    group: "Triage",
    enabled: (context) => context.selectedId !== null && !context.composerActive,
    run: (context) => context.openLabels(),
  },
  {
    id: "search.focus",
    title: "Search mail",
    keys: ["/"],
    group: "Application",
    enabled: () => true,
    run: (context) => context.focusSearch(),
  },
  {
    id: "mail.refresh",
    title: "Refresh mail",
    keys: ["Shift+r"],
    group: "Application",
    enabled: () => true,
    run: (context) => context.refresh(),
  },
  {
    id: "diagnostics.open",
    title: "Open sync diagnostics",
    keys: [],
    group: "Application",
    enabled: () => true,
    run: (context) => context.openDiagnostics(),
  },
];

export function matchesShortcut(event: KeyboardEvent, key: string): boolean {
  const parts = key.split("+");
  const base = parts.at(-1)!;
  return event.key.toLocaleLowerCase() === base.toLocaleLowerCase()
    && event.shiftKey === parts.includes("Shift")
    && (event.ctrlKey || event.metaKey) === parts.includes("Mod")
    && !event.altKey;
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
