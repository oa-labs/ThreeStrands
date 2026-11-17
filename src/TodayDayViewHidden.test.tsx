import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HOUR_HEIGHT } from "./calendarTime";
import type { ScheduleEvent } from "./domain";
import { hiddenEvents, TodayDayView } from "./TodayDayView";

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
  afterEach(() => cleanup());
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
});
