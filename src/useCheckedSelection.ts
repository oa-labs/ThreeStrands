import { useCallback, useEffect, useRef, useState } from "react";
import { applySelectionGesture, type SelectionGesture } from "./threadSelection";
import { useEscapeDismiss } from "./useEscapeDismiss";

/**
 * Checked rows for batch actions in a folder list, with the inbox's click,
 * ⌘-click and Shift-click rules. `orderedIds` are the rows that can be
 * checked, in visible order; ids that leave it are unchecked. Escape clears.
 */
export function useCheckedSelection(orderedIds: readonly string[], openId: string | null) {
  const [checkedIds, setCheckedIds] = useState<Set<string>>(() => new Set());
  const anchor = useRef<string | null>(null);

  useEffect(() => {
    setCheckedIds((current) => {
      if (current.size === 0) return current;
      const next = new Set([...current].filter((id) => orderedIds.includes(id)));
      return next.size === current.size ? current : next;
    });
  }, [orderedIds]);
  useEffect(() => { if (openId) anchor.current = openId; }, [openId]);
  useEscapeDismiss(() => setCheckedIds((current) => (current.size > 0 ? new Set() : current)), checkedIds.size > 0);

  const toggle = useCallback((id: string) => {
    anchor.current = id;
    setCheckedIds((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);
  const applyGesture = useCallback((id: string, gesture: SelectionGesture) => {
    const anchorId = anchor.current;
    if (gesture === "toggle") anchor.current = id;
    setCheckedIds((current) => applySelectionGesture({ gesture, targetId: id, anchorId, openId, orderedIds, checked: current }));
  }, [openId, orderedIds]);
  const selectAll = useCallback(() => setCheckedIds(new Set(orderedIds)), [orderedIds]);
  const clear = useCallback(() => setCheckedIds((current) => (current.size > 0 ? new Set() : current)), []);
  /** A plain click opens one row and ends any batch selection. */
  const open = useCallback((id: string) => {
    anchor.current = id;
    clear();
  }, [clear]);

  return { checkedIds, toggle, applyGesture, selectAll, clear, open };
}
