import { optionalFormattingHints, selectionFormattingHints } from "./richText";
import { formatShortcut } from "./shortcutLabels";

/** The same available selection formats in Mail and Notes. */
export function FormattingHints() {
  return <span className="compose-format-hints" aria-label="Formatting shortcuts">{selectionFormattingHints.map((shortcut) => <span key={shortcut.id} className={optionalFormattingHints[shortcut.id] ? `compose-format-hint-optional-${optionalFormattingHints[shortcut.id]}` : undefined}>{shortcut.title} <kbd>{formatShortcut(shortcut.key)}</kbd></span>)}</span>;
}
