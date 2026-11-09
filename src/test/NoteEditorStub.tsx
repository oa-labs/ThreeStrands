import { useImperativeHandle, useRef } from "react";
import type { NoteEditor as RealNoteEditor, NoteEditorHandle } from "../NoteEditor";

/** The props the stand-in editor last rendered with, so a test can follow a link or search as the picker would. */
export const lastEditorProps: { current: Parameters<typeof RealNoteEditor>[0] | null } = { current: null };

/**
 * A plain text box with the note editor's props, for tests of what
 * surrounds the editor. The editor itself is tested in NoteEditor.test.tsx.
 */
export function NoteEditor(props: Parameters<typeof RealNoteEditor>[0]) {
  const { ref, value, label, placeholder, readOnly = false, onChange, onLeave } = props;
  lastEditorProps.current = props;
  const box = useRef<HTMLTextAreaElement>(null);
  useImperativeHandle(ref, (): NoteEditorHandle => ({ focus: () => box.current?.focus(), markdown: () => value }), [value]);
  return <textarea ref={box} aria-label={label} value={value} placeholder={placeholder} readOnly={readOnly}
    onChange={(event) => onChange(event.target.value)}
    onKeyDown={(event) => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); onLeave(); } }} />;
}
