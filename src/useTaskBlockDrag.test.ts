import { act, fireEvent, renderHook } from "@testing-library/react";
import type { PointerEvent as ReactPointerEvent } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HOUR_HEIGHT } from "./calendarTime";
import { DAY_COLUMN_CLASS } from "./DayTimeGrid";
import { TASK_DRAG_SCROLL_STEP, TASK_DRAG_THRESHOLD, useTaskBlockDrag } from "./useTaskBlockDrag";

type Card = { id: string; title: string; kind: "card" };

const card: Card = { id: "card-1", title: "Plan launch", kind: "card" };
const monday = new Date(2026, 8, 21);
const tuesday = new Date(2026, 8, 22);

function press(x = 500, y = 10, pointerId = 1) {
  return { button: 0, pointerId, clientX: x, clientY: y } as ReactPointerEvent<HTMLElement>;
}

describe("useTaskBlockDrag", () => {
  let grid: HTMLDivElement;
  let columns: HTMLDivElement[];

  beforeEach(() => {
    grid = document.createElement("div");
    columns = [0, 1].map((index) => {
      const column = document.createElement("div");
      column.className = DAY_COLUMN_CLASS;
      column.dataset.dayIndex = String(index);
      vi.spyOn(column, "getBoundingClientRect").mockReturnValue({ top: 0 } as DOMRect);
      grid.append(column);
      return column;
    });
    vi.spyOn(grid, "getBoundingClientRect").mockReturnValue({ top: 0, bottom: 400, left: 0, right: 200 } as DOMRect);
    document.body.append(grid);
  });

  afterEach(() => {
    grid.remove();
    Reflect.deleteProperty(document, "elementFromPoint");
    vi.restoreAllMocks();
  });

  function renderDrag(days = [monday, tuesday], blockMinutes = 30) {
    const onDrop = vi.fn();
    const hook = renderHook(() => useTaskBlockDrag<Card>({ gridRef: { current: grid }, days, blockMinutes, onDrop }));
    return { ...hook, onDrop };
  }

  function pointAt(target: Element | null) {
    document.elementFromPoint = vi.fn(() => target);
  }

  it("previews and drops a generic source past the threshold on the day and slot under the pointer", () => {
    const { result, onDrop } = renderDrag();
    pointAt(columns[1]);
    act(() => result.current.startDrag(card, press()));
    expect(result.current.draggingId).toBeNull();

    fireEvent.pointerMove(document.body, { pointerId: 1, clientX: 100, clientY: 9.4 * HOUR_HEIGHT });
    expect(result.current.draggingId).toBe("card-1");
    // Snapped to the 15-minute slot, as long as the block.
    expect(result.current.preview).toEqual({ dayIndex: 1, startMinutes: 9 * 60 + 15, endMinutes: 9 * 60 + 45, label: "Plan launch" });

    fireEvent.pointerUp(document.body, { pointerId: 1, clientX: 100, clientY: 9.4 * HOUR_HEIGHT });
    expect(onDrop).toHaveBeenCalledExactlyOnceWith(card, tuesday, 9 * 60 + 15);
    expect(result.current.draggingId).toBeNull();
    expect(result.current.preview).toBeNull();
  });

  it("keeps a dropped block within the day", () => {
    const { result, onDrop } = renderDrag([monday], 30);
    pointAt(columns[0]);
    act(() => result.current.startDrag(card, press()));
    fireEvent.pointerMove(document.body, { pointerId: 1, clientX: 100, clientY: 24 * HOUR_HEIGHT });
    expect(result.current.preview).toMatchObject({ startMinutes: 23 * 60 + 30, endMinutes: 24 * 60 });
    fireEvent.pointerUp(document.body, { pointerId: 1, clientX: 100, clientY: 24 * HOUR_HEIGHT });
    expect(onDrop).toHaveBeenCalledWith(card, monday, 23 * 60 + 30);
  });

  it("treats a press that stays below the threshold as a click", () => {
    const { result, onDrop } = renderDrag();
    pointAt(columns[0]);
    act(() => result.current.startDrag(card, press(500, 10)));
    fireEvent.pointerMove(document.body, { pointerId: 1, clientX: 500 + TASK_DRAG_THRESHOLD - 1, clientY: 10 });
    expect(result.current.draggingId).toBeNull();
    expect(result.current.preview).toBeNull();
    fireEvent.pointerUp(document.body, { pointerId: 1, clientX: 500 + TASK_DRAG_THRESHOLD - 1, clientY: 10 });
    expect(onDrop).not.toHaveBeenCalled();
    expect(result.current.clickSuppressed()).toBe(false);
  });

  it("ignores other pointers and buttons", () => {
    const { result, onDrop } = renderDrag();
    pointAt(columns[0]);
    act(() => result.current.startDrag(card, { ...press(), button: 2 }));
    fireEvent.pointerMove(document.body, { pointerId: 1, clientX: 100, clientY: 100 });
    expect(result.current.draggingId).toBeNull();

    act(() => result.current.startDrag(card, press()));
    fireEvent.pointerMove(document.body, { pointerId: 2, clientX: 100, clientY: 100 });
    expect(result.current.draggingId).toBeNull();
    fireEvent.pointerUp(document.body, { pointerId: 2, clientX: 100, clientY: 100 });
    fireEvent.pointerUp(document.body, { pointerId: 1, clientX: 500, clientY: 10 });
    expect(onDrop).not.toHaveBeenCalled();
  });

  it("cancels on Escape without letting the key reach other handlers", () => {
    const { result, onDrop } = renderDrag();
    pointAt(columns[0]);
    act(() => result.current.startDrag(card, press()));
    fireEvent.pointerMove(document.body, { pointerId: 1, clientX: 100, clientY: 100 });
    expect(result.current.preview).not.toBeNull();

    const keydown = vi.fn();
    window.addEventListener("keydown", keydown);
    fireEvent.keyDown(document.body, { key: "Escape" });
    window.removeEventListener("keydown", keydown);
    expect(keydown).not.toHaveBeenCalled();
    expect(result.current.draggingId).toBeNull();
    expect(result.current.preview).toBeNull();

    fireEvent.pointerUp(document.body, { pointerId: 1, clientX: 100, clientY: 100 });
    expect(onDrop).not.toHaveBeenCalled();
  });

  it("does nothing for a drop outside the grid", () => {
    const outside = document.createElement("div");
    outside.className = DAY_COLUMN_CLASS;
    outside.dataset.dayIndex = "0";
    document.body.append(outside);
    const { result, onDrop } = renderDrag();
    act(() => result.current.startDrag(card, press()));

    pointAt(null);
    fireEvent.pointerMove(document.body, { pointerId: 1, clientX: 900, clientY: 100 });
    expect(result.current.draggingId).toBe("card-1");
    expect(result.current.preview).toBeNull();
    // A column-like element that isn't in this grid is not a target either.
    pointAt(outside);
    fireEvent.pointerUp(document.body, { pointerId: 1, clientX: 900, clientY: 100 });
    expect(onDrop).not.toHaveBeenCalled();
    outside.remove();
  });

  it("suppresses only the click that follows a drag", async () => {
    vi.useFakeTimers();
    try {
      const { result } = renderDrag();
      pointAt(null);
      act(() => result.current.startDrag(card, press()));
      fireEvent.pointerMove(document.body, { pointerId: 1, clientX: 100, clientY: 100 });
      fireEvent.pointerUp(document.body, { pointerId: 1, clientX: 100, clientY: 100 });
      expect(result.current.clickSuppressed()).toBe(true);
      await act(async () => { vi.runAllTimers(); });
      expect(result.current.clickSuppressed()).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it("scrolls the grid while the pointer is near its top or bottom edge", () => {
    const { result } = renderDrag();
    pointAt(columns[0]);
    grid.scrollTop = 100;
    act(() => result.current.startDrag(card, press()));
    fireEvent.pointerMove(document.body, { pointerId: 1, clientX: 100, clientY: 390 });
    expect(grid.scrollTop).toBe(100 + TASK_DRAG_SCROLL_STEP);
    fireEvent.pointerMove(document.body, { pointerId: 1, clientX: 100, clientY: 10 });
    expect(grid.scrollTop).toBe(100);
    fireEvent.pointerMove(document.body, { pointerId: 1, clientX: 100, clientY: 200 });
    expect(grid.scrollTop).toBe(100);
    // Beside the grid, an edge doesn't scroll it.
    fireEvent.pointerMove(document.body, { pointerId: 1, clientX: 300, clientY: 390 });
    expect(grid.scrollTop).toBe(100);
    fireEvent.keyDown(document.body, { key: "Escape" });
  });

  it("ends a drag in progress when unmounted", () => {
    const { result, unmount } = renderDrag();
    pointAt(columns[0]);
    act(() => result.current.startDrag(card, press()));
    unmount();
    const keydown = vi.fn();
    window.addEventListener("keydown", keydown);
    fireEvent.keyDown(document.body, { key: "Escape" });
    window.removeEventListener("keydown", keydown);
    expect(keydown).toHaveBeenCalled();
  });
});
