import { commands, shortcutSteps, type Command } from "./commands";
import { formattingShortcuts } from "./richText";

/**
 * The bound shortcuts shown in the in-app Keyboard Shortcuts dialog and on the
 * marketing site's shortcut reference, so the two can never disagree.
 */
export const SHORTCUT_GROUP_ORDER = ["Navigation", "Triage", "Compose", "Application"] as const;

export type ShortcutGroupName = (typeof SHORTCUT_GROUP_ORDER)[number];
export type ShortcutEntry = { id: string; title: string; keys: string[] };
export type ShortcutGroup = { group: ShortcutGroupName; entries: ShortcutEntry[] };

export function shortcutGroups(extraCommands: Command[] = []): ShortcutGroup[] {
  const entries = [
    ...[...commands, ...extraCommands].filter((command) => command.keys.length > 0),
    ...formattingShortcuts.map((shortcut) => ({ id: shortcut.id, title: shortcut.title, keys: [shortcut.key], group: "Compose" as const })),
  ];
  return SHORTCUT_GROUP_ORDER.map((group) => ({
    group,
    entries: entries.filter((entry) => entry.group === group).map(({ id, title, keys }) => ({ id, title, keys })),
  })).filter((group) => group.entries.length > 0);
}

/** Splits on "+" separators only, so a literal plus key ("Mod++") survives as its own part. */
export function formatShortcutStep(step: string): string {
  return step.split(/\+(?=.)/).map((part) => (part === "Mod" ? "⌘/Ctrl" : part)).join(" + ");
}

/** A shortcut's key presses in order, formatted for display ("g then i" → ["g", "i"]). */
export function shortcutDisplaySteps(key: string): string[] {
  return shortcutSteps(key).map(formatShortcutStep);
}
