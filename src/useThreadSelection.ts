import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
  type RefObject,
} from "react";
import type { Thread } from "./domain";
import { filterThreadsByMessageFilters, type MessageFilterKind } from "./messageFilters";
import { applySelectionGesture, type SelectionGesture } from "./threadSelection";
import { useEscapeDismiss } from "./useEscapeDismiss";

type Options = {
  threads: Thread[];
  selectedId: string | null;
  setSelectedId: Dispatch<SetStateAction<string | null>>;
  query: string;
  includeArchived: boolean;
  contextOpenedThreadRef: RefObject<string | null>;
};

export function useThreadSelection({ threads, selectedId, setSelectedId, query, includeArchived, contextOpenedThreadRef }: Options) {
  const [activeMessageFilters, setActiveMessageFilters] = useState<Set<MessageFilterKind>>(() => new Set());
  const toggleMessageFilter = useCallback((kind: MessageFilterKind) => {
    setActiveMessageFilters((current) => {
      const next = new Set(current);
      if (next.has(kind)) next.delete(kind);
      else next.add(kind);
      return next;
    });
  }, []);
  const [checkedIds, setCheckedIds] = useState<Set<string>>(new Set());
  const selectedThreadRowRef = useRef<HTMLButtonElement | null>(null);
  const selectAllRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    setCheckedIds(new Set());
  }, [query, includeArchived]);

  useEffect(() => {
    setCheckedIds((current) => {
      if (current.size === 0) return current;
      const next = new Set([...current].filter((id) => threads.some((thread) => thread.id === id)));
      return next.size === current.size ? current : next;
    });
  }, [threads]);

  // Claims Escape only while there is something to clear, so otherwise Escape can go back.
  useEscapeDismiss(() => {
    setCheckedIds((current) => (current.size > 0 ? new Set() : current));
    setActiveMessageFilters((current) => (current.size > 0 ? new Set() : current));
  }, checkedIds.size > 0 || activeMessageFilters.size > 0);

  const visibleThreads = useMemo(
    () => filterThreadsByMessageFilters(threads, activeMessageFilters),
    [threads, activeMessageFilters],
  );
  const selected = threads.find((thread) => thread.id === selectedId) ?? null;
  const selectedIndex = visibleThreads.findIndex((thread) => thread.id === selectedId);
  const selectionAnchorRef = useRef<string | null>(null);
  const visibleThreadIdsRef = useRef<string[]>([]);
  visibleThreadIdsRef.current = visibleThreads.map((thread) => thread.id);
  const selectedIdRef = useRef(selectedId);
  selectedIdRef.current = selectedId;
  useEffect(() => {
    selectionAnchorRef.current = selectedId;
  }, [selectedId]);
  const selectThread = useCallback((id: string) => {
    contextOpenedThreadRef.current = null;
    selectionAnchorRef.current = id;
    setCheckedIds((current) => (current.size > 0 ? new Set() : current));
    setSelectedId(id);
  }, [contextOpenedThreadRef, setSelectedId]);
  const applyThreadSelectionGesture = useCallback((id: string, gesture: SelectionGesture) => {
    const anchorId = selectionAnchorRef.current;
    if (gesture === "toggle") selectionAnchorRef.current = id;
    setCheckedIds((current) => applySelectionGesture({
      gesture,
      targetId: id,
      anchorId,
      openId: selectedIdRef.current,
      orderedIds: visibleThreadIdsRef.current,
      checked: current,
    }));
  }, []);
  const toggleChecked = useCallback((id: string) => {
    selectionAnchorRef.current = id;
    setCheckedIds((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);
  useEffect(() => {
    if (!selectAllRef.current) return;
    selectAllRef.current.indeterminate = checkedIds.size > 0 && checkedIds.size < threads.length;
  }, [checkedIds, threads.length]);

  useEffect(() => {
    selectedThreadRowRef.current?.scrollIntoView?.({ block: "nearest" });
  }, [selectedId]);

  return {
    checkedIds, setCheckedIds, selectedThreadRowRef, selectAllRef, activeMessageFilters,
    toggleMessageFilter, visibleThreads, selected, selectedIndex, selectThread,
    applyThreadSelectionGesture, toggleChecked,
  };
}
