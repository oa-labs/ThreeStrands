export type CommandContext = {
  selectedId: string | null;
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
  group: "Navigation" | "Triage" | "Application";
  enabled(context: CommandContext): boolean;
  run(context: CommandContext): void;
};

export const commands: Command[] = [
  {
    id: "thread.next",
    title: "Next conversation",
    keys: ["j", "ArrowDown"],
    group: "Navigation",
    enabled: () => true,
    run: (context) => context.selectNext(),
  },
  {
    id: "thread.previous",
    title: "Previous conversation",
    keys: ["k", "ArrowUp"],
    group: "Navigation",
    enabled: () => true,
    run: (context) => context.selectPrevious(),
  },
  {
    id: "thread.archive",
    title: "Archive",
    keys: ["e"],
    group: "Triage",
    enabled: (context) => context.selectedId !== null,
    run: (context) => context.archiveSelected(),
  },
  {
    id: "thread.read",
    title: "Toggle read",
    keys: ["u"],
    group: "Triage",
    enabled: (context) => context.selectedId !== null,
    run: (context) => context.toggleReadSelected(),
  },
  {
    id: "thread.star",
    title: "Toggle star",
    keys: ["s"],
    group: "Triage",
    enabled: (context) => context.selectedId !== null,
    run: (context) => context.toggleStarSelected(),
  },
  {
    id: "labels.open",
    title: "Manage labels",
    keys: ["v"],
    group: "Triage",
    enabled: (context) => context.selectedId !== null,
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
    keys: ["r"],
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
  return event.key.toLocaleLowerCase() === key.toLocaleLowerCase();
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
