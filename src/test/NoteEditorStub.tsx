import { useImperativeHandle, useRef } from "react";
import type { NoteEditor as RealNoteEditor, NoteEditorHandle } from "../NoteEditor";

/**
 * A plain text box with the note editor's props, for tests of what
 * surrounds the editor. The editor itself is tested in NoteEditor.test.tsx.
 */
export function NoteEditor({ ref, value, label, placeholder, readOnly = false, onChange, onLeave }: Parameters<typeof RealNoteEditor>[0]) {
  const box = useRef<HTMLTextAreaElement>(null);
  useImperativeHandle(ref, (): NoteEditorHandle => ({ focus: () => box.current?.focus(), markdown: () => value }), [value]);
  return <textarea ref={box} aria-label={label} value={value} placeholder={placeholder} readOnly={readOnly}
    onChange={(event) => onChange(event.target.value)}
    onKeyDown={(event) => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); onLeave(); } }} />;
}
