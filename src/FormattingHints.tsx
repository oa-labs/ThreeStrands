import { commands } from "./commands";
import { optionalFormattingHints, selectionFormattingHints } from "./richText";
import { formatShortcut } from "./shortcutLabels";

/** The same available selection formats in Mail and Notes. */
export function FormattingHints() {
  return <span className="compose-format-hints" aria-label="Formatting shortcuts">{selectionFormattingHints.map((shortcut) => <span key={shortcut.id} className={optionalFormattingHints[shortcut.id] ? `compose-format-hint-optional-${optionalFormattingHints[shortcut.id]}` : undefined}>{shortcut.title} <kbd>{formatShortcut(shortcut.key)}</kbd></span>)}</span>;
}

/** Labels for the compose commands worth reminding about while writing. */
const composeHintLabels: [id: string, label: string][] = [["draft.calendar", "Calendar"], ["draft.contextPanel", "Context Panel"]];

/** Compose shortcuts that work from inside the draft, shown while nothing is selected. */
export function ComposeShortcutHints() {
  return <span className="compose-format-hints" aria-label="Compose shortcuts">{composeHintLabels.map(([id, label]) => {
    const key = commands.find((command) => command.id === id)!.keys[0];
    return <span key={id}>{label} <kbd>{formatShortcut(key)}</kbd></span>;
  })}</span>;
}
