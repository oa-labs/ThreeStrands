import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CalendarAgenda, isPastEvent, TOMORROW_AGENDA_ROWS } from "./CalendarAgenda";
import type { CalendarOption, ScheduleEvent } from "./domain";

const calendars: CalendarOption[] = [{ id: "primary", accountId: "cal@example.com", name: "Work", primary: true, selected: true, writable: true }];
const day = new Date(2026, 9, 23);
const at = (hour: number, minute = 0) => new Date(2026, 9, 23, hour, minute);

function event(id: string, startHour: number, endHour: number, overrides: Partial<ScheduleEvent> = {}): ScheduleEvent {
  return {
    id, accountId: "cal@example.com", calendarId: "primary", title: id, allDay: false,
    start: at(startHour).toISOString(), end: at(endHour).toISOString(), ...overrides,
  };
}

function renderAgenda(events: ScheduleEvent[], props: { now?: Date; tomorrow?: boolean; today?: Date } = {}) {
  render(<CalendarAgenda
    today={props.today ?? day}
    now={props.now}
    tomorrow={props.tomorrow}
    calendars={calendars}
    onOpenEvent={vi.fn()}
    schedule={{ events, loading: false, error: null, reload: vi.fn() }}
  />);
}

/** The agenda's rows and divider, top to bottom, as their visible text. */
function agendaOrder(section: HTMLElement): string[] {
  return [...section.querySelectorAll(".context-row-title, .agenda-now")].map((node) =>
    node.classList.contains("agenda-now") ? "— now —" : node.textContent ?? "");
}

describe("CalendarAgenda for the current day", () => {
  afterEach(cleanup);

  it("dims finished events and puts a Now divider before the first one still to finish", () => {
    renderAgenda([
      event("Standup", 9, 10), event("Review", 15, 16), event("Holiday", 0, 0, { allDay: true, start: "2026-10-23", end: "2026-10-24" }),
      event("Lunch", 12, 13),
    ], { now: at(12, 30) });

    const section = screen.getByRole("region", { name: "Today's agenda" });
    expect(agendaOrder(section)).toEqual(["Holiday", "Standup", "— now —", "Lunch", "Review"]);
    expect(within(section).getByRole("separator", { name: /^Now, / })).toBeInTheDocument();
    expect(within(section).getByText("Standup").closest(".context-row")).toHaveClass("agenda-event-past");
    // An event under way, and all-day events, are not finished.
    expect(within(section).getByText("Lunch").closest(".context-row")).not.toHaveClass("agenda-event-past");
    expect(within(section).getByText("Holiday").closest(".context-row")).not.toHaveClass("agenda-event-past");
  });

  it("leaves the divider out before the day's first event has finished", () => {
    renderAgenda([event("Standup", 9, 10), event("Review", 15, 16)], { now: at(8) });
    expect(screen.queryByRole("separator")).not.toBeInTheDocument();
    expect(document.querySelector(".agenda-event-past")).toBeNull();
  });

  it("ends with the divider once every event has finished", () => {
    renderAgenda([event("Standup", 9, 10), event("Review", 15, 16)], { now: at(18) });
    expect(agendaOrder(screen.getByRole("region", { name: "Today's agenda" }))).toEqual(["Standup", "Review", "— now —"]);
  });

  it("marks nothing without the current time, as the Calendar view's agenda does", () => {
    renderAgenda([event("Standup", 9, 10)]);
    expect(screen.queryByRole("separator")).not.toBeInTheDocument();
    expect(document.querySelector(".agenda-event-past")).toBeNull();
  });

  it("counts an event as past only once it has ended", () => {
    expect(isPastEvent(event("a", 9, 10), at(10))).toBe(true);
    expect(isPastEvent(event("a", 9, 10), at(9, 59))).toBe(false);
    expect(isPastEvent(event("a", 0, 0, { allDay: true }), at(23))).toBe(false);
  });
});

describe("CalendarAgenda for tomorrow", () => {
  afterEach(cleanup);

  it(`shows tomorrow's first ${TOMORROW_AGENDA_ROWS} events, and the rest on request`, () => {
    const tomorrow = new Date(2026, 9, 24);
    const next = (id: string, hour: number) => ({
      ...event(id, 0, 0), start: new Date(2026, 9, 24, hour).toISOString(), end: new Date(2026, 9, 24, hour + 1).toISOString(),
    });
    renderAgenda([next("Dentist", 9), next("Gym", 7), next("Call", 11), next("Dinner", 19), event("Today only", 9, 10)], { today: tomorrow, tomorrow: true });

    const section = screen.getByRole("region", { name: "Tomorrow" });
    expect(agendaOrder(section)).toEqual(["Gym", "Dentist", "Call"]);
    fireEvent.click(within(section).getByRole("button", { name: "Show 1 more" }));
    expect(agendaOrder(section)).toEqual(["Gym", "Dentist", "Call", "Dinner"]);
  });

  it("says when tomorrow is free", () => {
    renderAgenda([], { today: new Date(2026, 9, 24), tomorrow: true });
    expect(within(screen.getByRole("region", { name: "Tomorrow" })).getByText("No events tomorrow.")).toBeInTheDocument();
  });
});
