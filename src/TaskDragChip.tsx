import { useLayoutEffect, useRef, type RefObject } from "react";
import { createPortal } from "react-dom";

/**
 * A small label with the dragged item's title that follows the pointer, so a
 * drag shows something being carried before it reaches a grid. It is moved
 * through the DOM rather than state, so a pointer move doesn't re-render the
 * view that owns the drag, and ignores the pointer so the grid under it still
 * receives hit tests.
 */
export function TaskDragChip({ title, pointerRef, onPointer }: {
  title: string | null;
  pointerRef: RefObject<{ x: number; y: number } | null>;
  onPointer: RefObject<((point: { x: number; y: number }) => void) | null>;
}) {
  const chip = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const place = (point: { x: number; y: number }) => { chip.current?.style.setProperty("transform", `translate(${point.x}px, ${point.y}px)`); };
    onPointer.current = place;
    if (pointerRef.current) place(pointerRef.current);
    return () => { onPointer.current = null; };
  }, [title, pointerRef, onPointer]);
  if (title === null) return null;
  return createPortal(<div ref={chip} className="task-drag-chip" aria-hidden="true">{title}</div>, document.body);
}
