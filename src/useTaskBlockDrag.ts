import { useCallback, useEffect, useLayoutEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type RefObject } from "react";
import { DAY_COLUMN_CLASS, slotAt, type DayGridPreview } from "./DayTimeGrid";

/** How far, in pixels, a source moves before it counts as dragged rather than clicked. */
export const TASK_DRAG_THRESHOLD = 4;
/** Distance from the grid's top or bottom edge, in pixels, that scrolls it during a drag. */
export const TASK_DRAG_SCROLL_EDGE = 40;
export const TASK_DRAG_SCROLL_STEP = 16;

/** Anything that can be dragged onto a day grid to book a block for it. */
export type TaskBlockDragSource = { id: string; title: string };

/**
 * Dragging a source onto a `DayTimeGrid` to book a block of `blockMinutes`.
 *
 * Owns the pointer mechanics only: the movement threshold, the drop preview,
 * scrolling near the grid's edges, Escape to cancel, and ignoring the click
 * that follows a drag. A drop on a day column calls `onDrop`; what a drop
 * books is the caller's business.
 */
export function useTaskBlockDrag<T extends TaskBlockDragSource>({
  gridRef,
  days,
  blockMinutes,
  onDrop,
}: {
  /** The grid's scrolling element, which holds the day columns. */
  gridRef: RefObject<HTMLElement | null>;
  /** The days the grid's columns show, in column order. */
  days: Date[];
  blockMinutes: number;
  onDrop(source: T, day: Date, startMinutes: number): void;
}): {
  /** The source being dragged, once it has passed the threshold. */
  draggingId: string | null;
  /** Where the block would land, for the grid to show. */
  preview: DayGridPreview | null;
  /** Call from the source's pointerdown. */
  startDrag(source: T, event: ReactPointerEvent<HTMLElement>): void;
  /** True for the click that a drag released over its source produces. */
  clickSuppressed(): boolean;
} {
  const [draggingId, setDraggingId] = useState<string | null>(null);
  const [preview, setPreview] = useState<DayGridPreview | null>(null);
  const dragRef = useRef<{ source: T; pointerId: number; x: number; y: number; moved: boolean } | null>(null);
  const endRef = useRef<(() => void) | null>(null);
  const suppressClick = useRef(false);
  const latest = useRef({ days, blockMinutes, onDrop });
  useLayoutEffect(() => { latest.current = { days, blockMinutes, onDrop }; });

  /** The day column under the pointer, and where a block dropped there starts. */
  const dropAt = useCallback((clientX: number, clientY: number) => {
    const target = document.elementFromPoint(clientX, clientY);
    const column = target instanceof Element ? target.closest<HTMLElement>(`.${DAY_COLUMN_CLASS}`) : null;
    if (!column || !gridRef.current?.contains(column)) return null;
    // Keep the block within the day.
    const startMinutes = Math.min(slotAt(clientY, column), 24 * 60 - latest.current.blockMinutes);
    return { dayIndex: Number(column.dataset.dayIndex), startMinutes };
  }, [gridRef]);

  const startDrag = useCallback((source: T, event: ReactPointerEvent<HTMLElement>) => {
    if (event.button !== 0 || dragRef.current) return;
    dragRef.current = { source, pointerId: event.pointerId, x: event.clientX, y: event.clientY, moved: false };
    const move = (pointer: PointerEvent) => {
      const drag = dragRef.current;
      if (!drag || pointer.pointerId !== drag.pointerId) return;
      if (!drag.moved) {
        if (Math.hypot(pointer.clientX - drag.x, pointer.clientY - drag.y) < TASK_DRAG_THRESHOLD) return;
        drag.moved = true;
        setDraggingId(drag.source.id);
      }
      pointer.preventDefault();
      const grid = gridRef.current;
      const bounds = grid?.getBoundingClientRect();
      if (grid && bounds && pointer.clientX >= bounds.left && pointer.clientX <= bounds.right) {
        if (pointer.clientY < bounds.top + TASK_DRAG_SCROLL_EDGE) grid.scrollTop -= TASK_DRAG_SCROLL_STEP;
        else if (pointer.clientY > bounds.bottom - TASK_DRAG_SCROLL_EDGE) grid.scrollTop += TASK_DRAG_SCROLL_STEP;
      }
      const drop = dropAt(pointer.clientX, pointer.clientY);
      setPreview(drop ? { ...drop, endMinutes: drop.startMinutes + latest.current.blockMinutes, label: drag.source.title } : null);
    };
    const finish = (pointer: PointerEvent) => {
      const drag = dragRef.current;
      if (!drag || pointer.pointerId !== drag.pointerId) return;
      end();
      if (!drag.moved) return;
      // The click that may follow a drag released over the source is not a request to activate it.
      suppressClick.current = true;
      window.setTimeout(() => { suppressClick.current = false; }, 0);
      const drop = dropAt(pointer.clientX, pointer.clientY);
      const day = drop ? latest.current.days[drop.dayIndex] : undefined;
      if (drop && day) latest.current.onDrop(drag.source, day, drop.startMinutes);
    };
    const cancel = (pointer: PointerEvent) => { if (pointer.pointerId === dragRef.current?.pointerId) end(); };
    const escape = (key: KeyboardEvent) => {
      if (key.key !== "Escape") return;
      key.preventDefault();
      key.stopPropagation();
      end();
    };
    const end = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", finish);
      window.removeEventListener("pointercancel", cancel);
      window.removeEventListener("keydown", escape, true);
      dragRef.current = null;
      endRef.current = null;
      setDraggingId(null);
      setPreview(null);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", finish);
    window.addEventListener("pointercancel", cancel);
    window.addEventListener("keydown", escape, true);
    endRef.current = end;
  }, [dropAt, gridRef]);
  useEffect(() => () => endRef.current?.(), []);

  const clickSuppressed = useCallback(() => suppressClick.current, []);

  return { draggingId, preview, startDrag, clickSuppressed };
}
