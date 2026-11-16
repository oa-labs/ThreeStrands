import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useRef } from "react";
import { DayTimeGrid } from "./DayTimeGrid";
import { useTaskBlockDrag } from "./useTaskBlockDrag";
import { HOUR_HEIGHT } from "./calendarTime";
import type { ScheduleEvent } from "./domain";

const today = new Date(2026, 8, 22);
const now = new Date(2026, 8, 22, 14, 30);

function event(id: string, start: string, end: string, overrides: Partial<ScheduleEvent> = {}): ScheduleEvent {
  return { id, accountId: "joel@example.com", calendarId: "primary", title: id, start, end, allDay: false, ...overrides };
}

const standup = event("Standup", "2026-09-22T09:00:00", "2026-09-22T09:30:00");
const review = event("Review", "2026-09-22T11:00:00", "2026-09-22T12:00:00");
const tomorrow = event("Tomorrow", "2026-09-23T10:00:00", "2026-09-23T11:00:00");
const offsite = event("Offsite", "2026-09-22", "2026-09-23", { allDay: true });

function renderGrid(props: Partial<Parameters<typeof DayTimeGrid>[0]> = {}) {
  const onToggleEvent = vi.fn();
  const view = render(
    <div>
      <DayTimeGrid days={[today]} events={[standup, review, tomorrow]} now={now} selectedEvent={null} onToggleEvent={onToggleEvent} {...props} />
    </div>,
  );
  return { ...view, onToggleEvent };
}

describe("DayTimeGrid", () => {
  afterEach(() => cleanup());

  it("renders one day's timed events and the Now line", () => {
    const { container } = renderGrid();
    const columns = container.querySelectorAll<HTMLElement>(".calendar-week-column");
    expect(columns).toHaveLength(1);
    expect(within(columns[0]).getAllByRole("button").map((button) => button.textContent)).toEqual([expect.stringContaining("Standup"), expect.stringContaining("Review")]);
    expect(within(columns[0]).getByRole("button", { name: /Review/ })).toHaveStyle({ top: `${11 * HOUR_HEIGHT}px`, height: `${HOUR_HEIGHT}px` });
    const nowLine = columns[0].querySelector<HTMLElement>("[data-testid='calendar-now-indicator']");
    expect(nowLine?.style.top).toBe(`${14.5 * HOUR_HEIGHT}px`);
    expect(container.querySelector(".calendar-week-day-today")).toHaveAttribute("aria-current", "date");
    expect(container.querySelector(".calendar-week-grid")).toHaveStyle({ "--calendar-day-count": "1" });
    expect(container.querySelector(".calendar-week-all-day")).not.toBeInTheDocument();
  });

  it("shows the Now line only on today's column", () => {
    const { container } = renderGrid({ days: [new Date(2026, 8, 23)] });
    expect(container.querySelector("[data-testid='calendar-now-indicator']")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Tomorrow/ })).toBeInTheDocument();
  });

  it("lists all-day events above the grid", () => {
    renderGrid({ events: [offsite, standup] });
    expect(within(screen.getByLabelText("All-day events")).getByRole("button", { name: "Offsite" })).toBeInTheDocument();
  });

  it("reports a clicked event with its trigger, marks the selected one, and opens context on Enter", () => {
    const onOpenEventContext = vi.fn();
    const { onToggleEvent } = renderGrid({ selectedEvent: review, onOpenEventContext });
    const standupButton = screen.getByRole("button", { name: /Standup/ });
    expect(screen.getByRole("button", { name: /Review/ })).toHaveAttribute("aria-expanded", "true");
    expect(standupButton).toHaveAttribute("aria-expanded", "false");

    fireEvent.click(standupButton);
    expect(onToggleEvent).toHaveBeenCalledExactlyOnceWith(standup, standupButton);
    fireEvent.keyDown(standupButton, { key: "Enter" });
    expect(onOpenEventContext).toHaveBeenCalledExactlyOnceWith(standup);
  });

  it("shows a drop preview with its label in the previewed column", () => {
    const { container } = renderGrid({ days: [today, new Date(2026, 8, 23)], preview: { dayIndex: 1, startMinutes: 9 * 60, endMinutes: 9 * 60 + 30, label: "Plan launch" } });
    const columns = container.querySelectorAll<HTMLElement>(".calendar-week-column");
    expect(columns[0].querySelector(".calendar-create-selection")).not.toBeInTheDocument();
    const preview = columns[1].querySelector(".calendar-create-selection");
    expect(preview).toHaveStyle({ top: `${9 * HOUR_HEIGHT}px`, height: `${HOUR_HEIGHT / 2}px` });
    expect(preview).toHaveTextContent("Plan launch");
  });

  it("gives each column its index and the caller's attributes", () => {
    const onPointerDown = vi.fn();
    const { container } = renderGrid({ days: [today, new Date(2026, 8, 23)], columnProps: (_day, dayIndex) => ({ "aria-label": `Column ${dayIndex}`, onPointerDown }) });
    const column = screen.getByLabelText("Column 1");
    expect(column).toHaveAttribute("data-day-index", "1");
    expect(column).toHaveClass("calendar-week-column");
    fireEvent.pointerDown(column);
    expect(onPointerDown).toHaveBeenCalledOnce();
    expect(container.querySelector(".calendar-grid-status")).not.toBeInTheDocument();
  });

  it("books a dragged card through the shared drag on a one-day grid", () => {
    const onDrop = vi.fn();
    function TodayHarness() {
      const gridRef = useRef<HTMLDivElement>(null);
      const days = [today];
      const drag = useTaskBlockDrag<{ id: string; title: string }>({ gridRef, days, blockMinutes: 30, onDrop });
      const card = { id: "card", title: "Plan launch" };
      return <>
        <button type="button" onPointerDown={(event) => drag.startDrag(card, event)}>Plan launch</button>
        <DayTimeGrid days={days} events={[]} now={now} selectedEvent={null} onToggleEvent={() => {}} preview={drag.preview} scrollRef={gridRef} />
      </>;
    }
    const { container } = render(<TodayHarness />);
    const column = container.querySelector<HTMLElement>(".calendar-week-column")!;
    vi.spyOn(column, "getBoundingClientRect").mockReturnValue({ top: 0 } as DOMRect);
    document.elementFromPoint = vi.fn(() => column);
    try {
      fireEvent.pointerDown(screen.getByRole("button", { name: "Plan launch" }), { button: 0, pointerId: 1, clientX: 900, clientY: 10 });
      fireEvent.pointerMove(document.body, { pointerId: 1, clientX: 50, clientY: 15 * HOUR_HEIGHT });
      expect(column.querySelector(".calendar-create-selection")).toHaveTextContent("Plan launch");
      fireEvent.pointerUp(document.body, { pointerId: 1, clientX: 50, clientY: 15 * HOUR_HEIGHT });
      expect(onDrop).toHaveBeenCalledExactlyOnceWith({ id: "card", title: "Plan launch" }, today, 15 * 60);
      expect(column.querySelector(".calendar-create-selection")).not.toBeInTheDocument();
    } finally {
      Reflect.deleteProperty(document, "elementFromPoint");
    }
  });
});
