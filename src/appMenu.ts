import { commands, type Command, type CommandContext } from "./commands";

/** The id of the menu item that is not a registry command. */
export const CHECK_FOR_UPDATES_ID = "app.checkForUpdates";

export type PredefinedName =
  | "About" | "Services" | "Hide" | "HideOthers" | "ShowAll" | "Quit" | "CloseWindow"
  | "Undo" | "Redo" | "Cut" | "Copy" | "Paste" | "SelectAll"
  | "Fullscreen" | "Minimize" | "Maximize" | "BringAllToFront";

export type MenuEntry =
  | { kind: "command"; id: string; title?: string }
  | { kind: "predefined"; item: PredefinedName; title?: string }
  | { kind: "separator" };

export type MenuSection = { title: string; role?: "window" | "help"; entries: MenuEntry[] };

const command = (id: string, title?: string): MenuEntry => ({ kind: "command", id, title });
const predefined = (item: PredefinedName, title?: string): MenuEntry => ({ kind: "predefined", item, title });
const separator: MenuEntry = { kind: "separator" };

/** The macOS menu bar. Titles, enabled state, and shortcuts come from the command registry. */
export const MENU_LAYOUT: MenuSection[] = [
  {
    title: "ThreeStrands",
    entries: [
      predefined("About"), separator,
      command("settings.open", "Settings…"),
      { kind: "command", id: CHECK_FOR_UPDATES_ID, title: "Check for Updates…" }, separator,
      predefined("Services"), separator,
      predefined("Hide"), predefined("HideOthers"), predefined("ShowAll"), separator,
      predefined("Quit"),
    ],
  },
  { title: "File", entries: [command("draft.new"), command("calendar.new"), separator, predefined("CloseWindow")] },
  {
    title: "Edit",
    entries: [
      predefined("Undo"), predefined("Redo"), separator,
      predefined("Cut"), predefined("Copy"), predefined("Paste"), predefined("SelectAll"), separator,
      command("search.focus"), command("palette.open"),
    ],
  },
  {
    title: "View",
    entries: [
      command("view.today"), command("view.mail"), command("view.calendar"), command("view.tasks"),
      command("view.notes"), command("view.contacts"), command("view.files"), command("view.agents"), separator,
      command("sidePanel.toggle"), separator,
      command("font.increase"), command("font.decrease"), command("theme.toggle"), separator,
      predefined("Fullscreen"),
    ],
  },
  {
    title: "Mailbox",
    entries: [
      command("mailbox.inbox"), command("snoozed.open"), command("mailbox.sent"), command("drafts.open"),
      command("outbox.open"), command("mailbox.allMail"), command("mailbox.trash"),
    ],
  },
  {
    title: "Message",
    entries: [
      command("draft.reply"), command("draft.replyAll"), command("draft.forward"), command("draft.editAsNew"), separator,
      command("thread.archive"), command("thread.snooze"), command("thread.trash"),
      command("thread.spam"), command("thread.notSpam"), command("thread.read"), command("thread.star"),
      command("labels.open", "Labels…"),
    ],
  },
  {
    title: "Window",
    role: "window",
    entries: [predefined("Minimize"), predefined("Maximize", "Zoom"), separator, predefined("BringAllToFront")],
  },
  {
    title: "Help",
    role: "help",
    entries: [command("help.open"), command("gettingStarted.open"), command("shortcuts.open")],
  },
];

/** Shortcuts the webview owns in context, or that the native Edit menu already claims. */
export const UNACCELERATED_SHORTCUTS = ["Mod+Enter", "Mod+a", "Mod+z"];

/**
 * A native accelerator for a registry binding, or null. macOS menus can only
 * express a single key chord with a Cmd modifier, so single keys and sequences
 * ("g then i") get none.
 */
export function toAccelerator(key: string): string | null {
  if (!key.startsWith("Mod+") || key.includes(" then ")) return null;
  if (UNACCELERATED_SHORTCUTS.some((excluded) => excluded.toLowerCase() === key.toLowerCase())) return null;
  // "Mod++" is Mod plus the plus key.
  return `CmdOrCtrl+${key.slice(4)}`;
}

export function commandAccelerator(item: Pick<Command, "keys">): string | null {
  for (const key of item.keys) {
    const accelerator = toAccelerator(key);
    if (accelerator) return accelerator;
  }
  return null;
}

export type ResolvedCommandItem = { id: string; title: string; accelerator: string | null };

export function resolveCommandItem(entry: Extract<MenuEntry, { kind: "command" }>): ResolvedCommandItem {
  if (entry.id === CHECK_FOR_UPDATES_ID) return { id: entry.id, title: entry.title ?? "Check for Updates…", accelerator: null };
  const found = commands.find((candidate) => candidate.id === entry.id);
  if (!found) throw new Error(`Menu references unknown command "${entry.id}"`);
  return { id: entry.id, title: entry.title ?? found.title, accelerator: commandAccelerator(found) };
}

/** Every command entry in the layout, in order. */
export function layoutCommandEntries(layout: MenuSection[] = MENU_LAYOUT) {
  return layout.flatMap((section) => section.entries).filter((entry): entry is Extract<MenuEntry, { kind: "command" }> => entry.kind === "command");
}

/**
 * Menu items follow the keyboard rules: nothing runs while the app is closing
 * or while a dialog or the palette holds the keyboard.
 */
export function menuItemEnabled(id: string, context: CommandContext): boolean {
  if (context.closing || context.interactionScope === "modal" || context.interactionScope === "palette") return false;
  if (id === CHECK_FOR_UPDATES_ID) return true;
  const found = commands.find((candidate) => candidate.id === id);
  return Boolean(found?.enabled(context));
}

export function menuEnabledStates(context: CommandContext, layout: MenuSection[] = MENU_LAYOUT): Map<string, boolean> {
  return new Map(layoutCommandEntries(layout).map((entry) => [entry.id, menuItemEnabled(entry.id, context)]));
}

/** The entries whose enabled state differs from what the native menu last received. */
export function diffEnabled(previous: ReadonlyMap<string, boolean>, next: ReadonlyMap<string, boolean>): [string, boolean][] {
  return [...next].filter(([id, enabled]) => previous.get(id) !== enabled);
}
