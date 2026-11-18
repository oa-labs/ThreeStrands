import type { EditorHandle } from "@meowdown/react";

/** Clear source-backed inline styles, keeping unselected text and undo intact. */
export function clearNoteFormatting(handle: EditorHandle): void {
  const editor = handle.getEditor();
  if (!editor || editor.state.selection.empty) return;
  const view = editor.view;
  const initial = editor.state;
  const all = initial.selection.toJSON().type === "all";
  const combined = initial.tr;
  let finalSelection: typeof initial.selection;
  let { anchor, head } = initial.selection;
  const originalDispatch = view.dispatch;
  // Each library toggle edits Markdown delimiters and maps its selection.
  // Run those commands on temporary editor states, then commit their steps
  // together so mixed formatting clears as one undoable edit.
  view.dispatch = (transaction) => {
    const result = view.state.applyTransaction(transaction.setMeta("addToHistory", false));
    for (const applied of result.transactions) {
      const forward = anchor <= head;
      anchor = applied.mapping.map(anchor, forward ? 1 : -1);
      head = applied.mapping.map(head, forward ? -1 : 1);
      for (const step of applied.steps) combined.step(step);
    }
    view.updateState(result.state);
  };
  try {
    const formats = [
      ["mdStrong", () => editor.commands.toggleStrong()],
      ["mdEm", () => editor.commands.toggleEm()],
      ["mdDel", () => editor.commands.toggleDel()],
      ["mdHighlight", () => editor.commands.toggleHighlight()],
      ["mdCode", () => editor.commands.toggleCode()],
      ["mdLinkText", () => editor.commands.removeLink()],
    ] as const;
    for (const [mark, remove] of formats) {
      for (;;) {
        let span: { anchor: number; head: number } | undefined;
        editor.state.doc.nodesBetween(Math.min(anchor, head), Math.max(anchor, head), (node, pos) => {
          if (span || !node.isText || !node.text?.trim() || node.marks.some((m) => m.type.name === "mdMark")) return;
          if (node.marks.some((m) => m.type.name === mark)) span = {
            anchor: Math.max(pos, Math.min(anchor, head)), head: Math.min(pos + node.nodeSize, Math.max(anchor, head)),
          };
        });
        if (!span) break;
        handle.setSelection({ type: "text", ...span });
        const before = editor.state.doc;
        if (!remove() || before.eq(editor.state.doc)) break;
      }
    }
    handle.setSelection({ type: all ? "all" : "text", anchor, head });
    finalSelection = editor.state.selection.getBookmark().resolve(combined.doc);
  } finally {
    view.dispatch = originalDispatch;
    view.updateState(initial);
  }
  if (combined.docChanged) originalDispatch.call(view, combined.setSelection(finalSelection).scrollIntoView());
}
