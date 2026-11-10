/**
 * The one notation for showing a key binding to a reader. Bindings are
 * written in the registry form ("Mod+Shift+l", "g then i"); this turns one
 * step of a binding into display text.
 *
 * - "mac": the glyphs and order of macOS menus, with no separators ("⌘⇧L").
 * - "other": Windows and Linux text joined with "+" ("Ctrl+Shift+L").
 * - "any": for readers whose platform is unknown, such as the marketing site,
 *   in the form its hand-written docs use ("⌘/Ctrl + Shift + l").
 *
 * On "mac" and "other", a letter is capitalized after a modifier, as on the
 * keycap, and left as typed when pressed alone, where "l" and "L" are
 * different bindings.
 */
export type ShortcutPlatform = "mac" | "other" | "any";

export function currentShortcutPlatform(platform = typeof navigator === "undefined" ? "" : navigator.platform): Exclude<ShortcutPlatform, "any"> {
  return /Mac|iPhone|iPad|iPod/i.test(platform) ? "mac" : "other";
}

const MAC_GLYPHS: Record<string, string> = { Mod: "⌘", Meta: "⌘", Ctrl: "⌃", Control: "⌃", Alt: "⌥", Option: "⌥", Shift: "⇧", Enter: "↵" };
const TEXT_NAMES: Record<string, string> = { Mod: "Ctrl", Meta: "Ctrl", Control: "Ctrl", Option: "Alt" };
const SHARED_NAMES: Record<string, string> = { Escape: "Esc", ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→", " ": "Space" };

/** Splits on "+" separators only, so a literal plus key ("Mod++") survives as its own part. */
function parts(step: string): string[] {
  return step.split(/\+(?=.)/);
}

/** A whole binding, including sequences ("g then i"), as one line of text. */
export function formatShortcut(binding: string, platform: ShortcutPlatform = currentShortcutPlatform()): string {
  return binding.split(" then ").map((step) => formatShortcutStep(step, platform)).join(" then ");
}

/** Modifier order: macOS menus read ⌃⌥⇧⌘; Windows and Linux read Ctrl+Alt+Shift. */
const MAC_ORDER: Record<string, number> = { Ctrl: 0, Control: 0, Alt: 1, Option: 1, Shift: 2, Mod: 3, Meta: 3 };
const TEXT_ORDER: Record<string, number> = { Mod: 0, Meta: 0, Ctrl: 0, Control: 0, Alt: 1, Option: 1, Shift: 2 };

export function formatShortcutStep(step: string, platform: ShortcutPlatform = currentShortcutPlatform()): string {
  const keys = parts(step);
  if (platform === "any") return keys.map((key) => (key === "Mod" ? "⌘/Ctrl" : key)).join(" + ");
  const key = keys.at(-1)!;
  const order = platform === "mac" ? MAC_ORDER : TEXT_ORDER;
  const modifiers = keys.slice(0, -1).sort((a, b) => (order[a] ?? 9) - (order[b] ?? 9));
  const names = platform === "mac" ? MAC_GLYPHS : TEXT_NAMES;
  const shown = [...modifiers, key].map((part, index, all) => {
    const named = names[part] ?? SHARED_NAMES[part];
    if (named) return named;
    return index === all.length - 1 && modifiers.length > 0 && part.length === 1 ? part.toUpperCase() : part;
  });
  return shown.join(platform === "mac" ? "" : "+");
}
