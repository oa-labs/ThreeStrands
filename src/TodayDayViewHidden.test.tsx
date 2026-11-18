import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HOUR_HEIGHT } from "./calendarTime";
import type { ScheduleEvent } from "./domain";
import { DAY_VIEW_LABEL_ROOM_PX, hiddenEvents, TodayDayView } from "./TodayDayView";

const hour = (h: number, length = 60) => ({ startMinutes: h * 60, endMinutes: h * 60 + length });
const view = 6 * HOUR_HEIGHT;

describe("hiddenEvents", () => {
  it("counts events wholly below or above the viewport and reports the earliest of each", () => {
    expect(hiddenEvents([hour(1), hour(9), hour(18, 30), hour(19), hour(22)], 8 * HOUR_HEIGHT, view)).toEqual({
      above: { count: 1, firstMinutes: 60 },
      below: { count: 3, firstMinutes: 18 * 60 },
    });
  });

  it("treats an event that touches the viewport as visible and one exactly past its edge as hidden", () => {
    const top = 8 * HOUR_HEIGHT;
    // Ends at the top edge: hidden. Starts at the bottom edge: hidden. One pixel in: visible.
    expect(hiddenEvents([hour(7)], top, view).above?.count).toBe(1);
    expect(hiddenEvents([{ startMinutes: 7 * 60 + 1, endMinutes: 8 * 60 + 1 }], top, view).above).toBeNull();
    expect(hiddenEvents([hour(14)], top, view).below?.count).toBe(1);
    expect(hiddenEvents([{ startMinutes: 14 * 60 - 1, endMinutes: 15 * 60 }], top, view).below).toBeNull();
  });

  it("reports nothing without a measured viewport", () => {
    expect(hiddenEvents([hour(20)], 0, 0)).toEqual({ above: null, below: null });
  });
});

describe("TodayDayView hidden-event indicators", () => {
  beforeEach(() => {
    // Initial scrolling captures the opening time, separately from the minute-by-minute clock.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2026, 9, 28, 10));
  });
  afterEach(() => { cleanup(); vi.useRealTimers(); vi.unstubAllGlobals(); });
  const day = new Date(2026, 9, 28);
  const meeting = (id: string, h: number): ScheduleEvent => ({
    id, accountId: "a@example.com", calendarId: "primary", title: id,
    start: new Date(2026, 9, 28, h).toISOString(), end: new Date(2026, 9, 28, h + 1).toISOString(), allDay: false,
  });

  function renderView(events: ScheduleEvent[]) {
    const gridRef = createRef<HTMLDivElement>();
    const proto = HTMLElement.prototype;
    const client = vi.spyOn(proto, "clientHeight", "get").mockReturnValue(view);
    render(<TodayDayView events={events} now={new Date(2026, 9, 28, 10)} day={day} loading={false} error={null} onRetry={vi.fn()}
      selectedEvent={null} onChooseEvent={vi.fn()} eventDecoration={() => null} gridRef={gridRef} preview={null} dragging={false} />);
    return { gridRef, restore: () => client.mockRestore() };
  }

  it("points to evening meetings below the view and scrolls to the first when activated", () => {
    const { gridRef, restore } = renderView([meeting("Standup", 9), meeting("Dinner", 19), meeting("Late call", 21)]);
    expect(screen.getByRole("group", { name: "Day view" }).querySelector(".calendar-week-timezone")).not.toBeInTheDocument();
    const grid = gridRef.current!;
    fireEvent.scroll(grid);
    const below = screen.getByRole("button", { name: /2 more later, from 7:00/ });
    expect(screen.queryByRole("button", { name: /earlier/ })).not.toBeInTheDocument();
    fireEvent.click(below);
    expect(grid.scrollTop).toBe(19 * HOUR_HEIGHT - HOUR_HEIGHT / 2);
    restore();
  });

  it("points to meetings scrolled out above and drops the pointer once they are in view", () => {
    const { gridRef, restore } = renderView([meeting("Early", 6), meeting("Standup", 15)]);
    const grid = gridRef.current!;
    grid.scrollTop = 12 * HOUR_HEIGHT;
    fireEvent.scroll(grid);
    const above = screen.getByRole("button", { name: /1 earlier, from 6:00/ });
    fireEvent.click(above);
    expect(grid.scrollTop).toBe(6 * HOUR_HEIGHT - HOUR_HEIGHT / 2);
    expect(screen.queryByRole("button", { name: /earlier/ })).not.toBeInTheDocument();
    restore();
  });

  it("updates hidden-event indicators when the column grows without moving the user's scroll position", () => {
    let notify!: () => void;
    const observe = vi.fn();
    const disconnect = vi.fn();
    vi.stubGlobal("ResizeObserver", class {
      constructor(callback: () => void) { notify = callback; }
      observe = observe;
      disconnect = disconnect;
    });
    const { gridRef, restore } = renderView([meeting("Standup", 9), meeting("Dinner", 19), meeting("Late call", 21)]);
    const grid = gridRef.current!;
    expect(observe).toHaveBeenCalledWith(grid);
    expect(screen.getByRole("button", { name: /2 more later/ })).toBeInTheDocument();
    const before = grid.scrollTop;
    Object.defineProperty(grid, "clientHeight", { configurable: true, value: 16 * HOUR_HEIGHT });
    act(() => notify());
    expect(screen.queryByRole("button", { name: /more later/ })).not.toBeInTheDocument();
    expect(grid.scrollTop).toBe(before);
    restore();
  });
});

describe("TodayDayView creating events", () => {
  afterEach(cleanup);
  const day = new Date(2026, 9, 28);
  const meeting: ScheduleEvent = { id: "m", accountId: "a@example.com", calendarId: "primary", title: "Standup", start: new Date(2026, 9, 28, 9).toISOString(), end: new Date(2026, 9, 28, 10).toISOString(), allDay: false };
  function renderCreating(props: Partial<Parameters<typeof TodayDayView>[0]> = {}) {
    const onCreateEvent = vi.fn();
    const onChooseEvent = vi.fn();
    render(<TodayDayView events={[meeting]} now={new Date(2026, 9, 28, 10)} day={day} loading={false} error={null} onRetry={vi.fn()}
      selectedEvent={null} onChooseEvent={onChooseEvent} eventDecoration={() => null} gridRef={createRef<HTMLDivElement>()} preview={null} dragging={false}
      onCreateEvent={onCreateEvent} {...props} />);
    return { onCreateEvent, onChooseEvent, column: screen.getByRole("group", { name: /Wednesday/ }) };
  }
  const press = (column: HTMLElement, minutes: number, extra = {}) => fireEvent.pointerDown(column, { button: 0, pointerId: 1, clientY: minutes / 60 * HOUR_HEIGHT, ...extra });
  const release = (column: HTMLElement, minutes: number) => fireEvent.pointerUp(column, { pointerId: 1, clientY: minutes / 60 * HOUR_HEIGHT });

  it("makes a one-hour range from a click, snapped to the grid", () => {
    const { onCreateEvent, column } = renderCreating();
    press(column, 13 * 60 + 20);
    release(column, 13 * 60 + 20);
    expect(onCreateEvent).toHaveBeenCalledExactlyOnceWith({ start: new Date(2026, 9, 28, 13, 15), end: new Date(2026, 9, 28, 14, 15) });
  });

  it("makes a range from a drag, ending with the last slot, and previews it while dragging", () => {
    const { onCreateEvent, column } = renderCreating();
    press(column, 14 * 60);
    fireEvent.pointerMove(column, { pointerId: 1, clientY: 15 * HOUR_HEIGHT + 20 });
    expect(column.querySelector(".calendar-create-selection")).not.toBeNull();
    release(column, 15 * 60 + 20);
    expect(onCreateEvent).toHaveBeenCalledExactlyOnceWith({ start: new Date(2026, 9, 28, 14), end: new Date(2026, 9, 28, 15, 30) });
    expect(column.querySelector(".calendar-create-selection")).toBeNull();
  });

  it("leaves events, a task being dragged, and a cancelled gesture alone", () => {
    const { onCreateEvent, onChooseEvent, column } = renderCreating();
    const standup = screen.getByRole("button", { name: /^Standup/ });
    fireEvent.pointerDown(standup, { button: 0, pointerId: 1, clientY: 9.5 * HOUR_HEIGHT });
    fireEvent.pointerUp(column, { pointerId: 1, clientY: 9.5 * HOUR_HEIGHT });
    fireEvent.click(standup);
    expect(onChooseEvent).toHaveBeenCalledOnce();
    press(column, 12 * 60);
    fireEvent.pointerCancel(column, { pointerId: 1 });
    release(column, 12 * 60);
    expect(onCreateEvent).not.toHaveBeenCalled();
    cleanup();
    const dragging = renderCreating({ dragging: true });
    press(dragging.column, 12 * 60);
    release(dragging.column, 12 * 60);
    expect(dragging.onCreateEvent).not.toHaveBeenCalled();
  });

  it("does nothing without a handler, and names the shaded hours", () => {
    const { column } = renderCreating({ onCreateEvent: undefined, workingHours: { timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC", workingWindows: [{ weekday: 3, start: "09:00", end: "17:00" }] } });
    press(column, 12 * 60);
    expect(column.querySelector(".calendar-create-selection")).toBeNull();
    expect(column).toHaveAccessibleDescription("Shaded time is outside working hours");
    expect(column.querySelector(".calendar-outside-hours")).toHaveAttribute("title", "Outside working hours");
  });

  it("leaves room above the first working hour so its label is not cut off", () => {
    const gridRef = createRef<HTMLDivElement>();
    render(<TodayDayView events={[]} now={new Date(2026, 9, 28, 7)} day={day} loading={false} error={null} onRetry={vi.fn()} selectedEvent={null} onChooseEvent={vi.fn()}
      eventDecoration={() => null} gridRef={gridRef} preview={null} dragging={false} workingHours={{ timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC", workingWindows: [{ weekday: 3, start: "09:00", end: "17:00" }] }} />);
    expect(gridRef.current!.scrollTop).toBe(9 * HOUR_HEIGHT - DAY_VIEW_LABEL_ROOM_PX);
  });
});
