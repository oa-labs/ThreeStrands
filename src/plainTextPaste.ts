import type { KeyboardEvent } from "react";
import { errorMessage } from "./errors";

/** Read only text, directly in the keyboard gesture, including in WebKit. */
export function pasteWithoutFormatting(
  event: KeyboardEvent<HTMLElement>,
  insert: (text: string, editor: HTMLElement) => void,
  onError: (message: string) => void,
): boolean {
  if (!(event.metaKey || event.ctrlKey) || !event.shiftKey || event.altKey || event.key.toLowerCase() !== "v") return false;
  const editor = event.target instanceof Element ? event.target.closest<HTMLElement>("[contenteditable]") : null;
  if (!editor || editor.getAttribute("contenteditable") !== "true" || !event.currentTarget.contains(editor)) return false;
  event.preventDefault();
  event.stopPropagation();
  if (event.repeat || event.nativeEvent.isComposing) return true;

  const selection = window.getSelection();
  if (!selection?.rangeCount || !editor.contains(selection.anchorNode) || !editor.contains(selection.focusNode)) return true;
  const range = selection.getRangeAt(0).cloneRange();
  const originalHtml = editor.innerHTML;
  const stillCurrent = () => {
    const current = window.getSelection();
    if (!editor.isConnected || editor.getAttribute("contenteditable") !== "true" || !editor.contains(document.activeElement)
      || editor.innerHTML !== originalHtml || !current?.rangeCount) return false;
    const next = current.getRangeAt(0);
    return next.startContainer === range.startContainer && next.startOffset === range.startOffset
      && next.endContainer === range.endContainer && next.endOffset === range.endOffset;
  };
  try {
    // Cancel native paste to avoid a second insertion. Ordinary Mod+V never
    // enters this path and retains each editor's existing clipboard behavior.
    void navigator.clipboard.readText().then((text) => {
      // A clipboard prompt may outlive this editor or a change of selection.
      if (text && stillCurrent()) insert(text.replace(/\r\n?/g, "\n"), editor);
    }).catch((reason: unknown) => {
      if (stillCurrent()) onError(`Could not paste without formatting: ${errorMessage(reason)}`);
    });
  } catch (reason) {
    onError(`Could not paste without formatting: ${errorMessage(reason)}`);
  }
  return true;
}
