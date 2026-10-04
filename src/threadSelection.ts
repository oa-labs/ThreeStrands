export type SelectionGesture = "toggle" | "range" | "range-add";

export function selectionGestureFor(event: { metaKey: boolean; ctrlKey: boolean; shiftKey: boolean }): SelectionGesture | null {
  const toggle = event.metaKey || event.ctrlKey;
  if (event.shiftKey) return toggle ? "range-add" : "range";
  return toggle ? "toggle" : null;
}

/**
 * Standard list multi-select over the checked set. A toggle on an empty set
 * seeds it with the open conversation, so ⌘-click after a plain click selects
 * both. A range runs from the anchor to the target in visible order; plain
 * Shift replaces the set, ⌘⇧ adds to it.
 */
export function applySelectionGesture({
  gesture,
  targetId,
  anchorId,
  openId,
  orderedIds,
  checked,
}: {
  gesture: SelectionGesture;
  targetId: string;
  anchorId: string | null;
  openId: string | null;
  orderedIds: readonly string[];
  checked: ReadonlySet<string>;
}): Set<string> {
  if (gesture === "toggle") {
    const next = new Set(checked);
    if (next.size === 0 && openId && openId !== targetId && orderedIds.includes(openId)) next.add(openId);
    if (next.has(targetId)) next.delete(targetId);
    else next.add(targetId);
    return next;
  }

  const targetIndex = orderedIds.indexOf(targetId);
  const anchorIndex = anchorId ? orderedIds.indexOf(anchorId) : -1;
  const next = gesture === "range-add" ? new Set(checked) : new Set<string>();
  if (targetIndex === -1) return next;
  if (anchorIndex === -1) {
    next.add(targetId);
    return next;
  }
  const [start, end] = anchorIndex <= targetIndex ? [anchorIndex, targetIndex] : [targetIndex, anchorIndex];
  for (const id of orderedIds.slice(start, end + 1)) next.add(id);
  return next;
}
