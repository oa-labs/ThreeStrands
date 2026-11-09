import type { RefObject } from "react";

/**
 * The note's writing surface. It edits the Markdown source as text; the
 * stored value is always Markdown, so a richer editor can replace this one
 * behind the same props. Escape hands focus back to the note list.
 */
export function NoteEditor({ ref, value, label, placeholder, readOnly = false, onChange, onBlur, onLeave }: {
  ref?: RefObject<HTMLTextAreaElement | null>;
  value: string;
  label: string;
  placeholder?: string;
  readOnly?: boolean;
  onChange(value: string): void;
  onBlur?(): void;
  onLeave(): void;
}) {
  return <textarea
    ref={ref}
    className="note-editor-body"
    aria-label={label}
    value={value}
    placeholder={placeholder}
    readOnly={readOnly}
    spellCheck
    onChange={(event) => onChange(event.target.value)}
    onBlur={onBlur}
    onKeyDown={(event) => {
      if (event.key !== "Escape" || event.nativeEvent.isComposing) return;
      event.preventDefault();
      event.stopPropagation();
      onLeave();
    }}
  />;
}
