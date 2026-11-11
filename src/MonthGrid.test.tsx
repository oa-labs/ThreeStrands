import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { firstDayOfWeek, MonthGrid, monthGridRange } from "./MonthGrid";

const long = new Intl.DateTimeFormat(undefined, { weekday: "long", month: "long", day: "numeric", year: "numeric" });
const name = (date: string) => long.format(new Date(`${date}T12:00:00`));

function Grid({ start = "2026-11-11", weekStart, marked, onSelect = vi.fn() }: { start?: string; weekStart?: number; marked?: Set<string>; onSelect?(date: string): void }) {
  const [cursor, setCursor] = useState(() => new Date(`${start}T12:00:00`));
  return <>
    <span id="month">Month</span>
    <button type="button" onClick={() => setCursor(new Date("2026-12-11T12:00:00"))}>Show December</button>
    <MonthGrid labelledBy="month" cursor={cursor} onCursorChange={setCursor} selected={start} onSelect={onSelect} weekStart={weekStart} marked={marked} markLabel="has a note" />
  </>;
}

afterEach(cleanup);

describe("MonthGrid", () => {
  it("starts the week where the locale does, and on Sunday when it can't tell", () => {
    expect(firstDayOfWeek("en-US")).toBe(0);
    expect(firstDayOfWeek("en-GB")).toBe(1);
    expect(firstDayOfWeek("not a locale!")).toBe(0);
  });

  it("lays out only the weeks that hold the month, from the chosen first weekday", () => {
    expect(monthGridRange(new Date("2026-11-11T12:00:00"), 1)).toMatchObject({ weeks: 6, from: "2026-10-26", to: "2026-12-06" });
    expect(monthGridRange(new Date("2026-11-11T12:00:00"), 0)).toMatchObject({ weeks: 5, from: "2026-11-01", to: "2026-12-05" });
    // February 2026 starts on a Sunday and fills four weeks exactly.
    expect(monthGridRange(new Date("2026-02-11T12:00:00"), 0)).toMatchObject({ weeks: 4, from: "2026-02-01", to: "2026-02-28" });
    render(<Grid weekStart={1} />);
    expect(screen.getAllByRole("columnheader")[0]).toHaveAttribute("abbr", "Monday");
    expect(screen.getAllByRole("gridcell")[0]).toHaveTextContent(/^26$/);
    expect(screen.getAllByRole("row")).toHaveLength(7);
    cleanup();
    render(<Grid weekStart={0} />);
    expect(screen.getAllByRole("row")).toHaveLength(6);
    expect(screen.queryByRole("button", { name: name("2026-12-06") })).not.toBeInTheDocument();
  });

  it("moves Home and End to the ends of a Monday week", () => {
    render(<Grid weekStart={1} start="2026-11-08" />);
    const grid = screen.getByRole("grid", { name: "Month" });
    screen.getByRole("button", { name: name("2026-11-08") }).focus();
    fireEvent.keyDown(grid, { key: "Home" });
    expect(screen.getByRole("button", { name: name("2026-11-02") })).toHaveFocus();
    fireEvent.keyDown(grid, { key: "End" });
    expect(screen.getByRole("button", { name: name("2026-11-08") })).toHaveFocus();
  });

  it("names marked days for screen readers and chooses a day on click", () => {
    const onSelect = vi.fn();
    render(<Grid weekStart={1} marked={new Set(["2026-11-09", "2026-10-27"])} onSelect={onSelect} />);
    // A neighbouring month's day carries no mark.
    const outside = screen.getByRole("button", { name: name("2026-10-27") });
    expect(outside).toHaveClass("month-grid-outside-month");
    expect(outside).not.toHaveClass("month-grid-day-marked");
    const marked = screen.getByRole("button", { name: `${name("2026-11-09")}, has a note` });
    expect(marked).toHaveClass("month-grid-day-marked");
    expect(screen.getByRole("button", { name: name("2026-11-10") })).not.toHaveClass("month-grid-day-marked");
    fireEvent.click(marked);
    expect(onSelect).toHaveBeenCalledExactlyOnceWith("2026-11-09");
  });

  it("takes focus only for keyboard moves when it sits on the page", () => {
    render(<Grid />);
    expect(document.body).toHaveFocus();
    const other = screen.getByRole("button", { name: "Show December" });
    other.focus();
    fireEvent.click(other);
    expect(other).toHaveFocus();
    fireEvent.keyDown(screen.getByRole("grid"), { key: "ArrowLeft" });
    expect(screen.getByRole("button", { name: name("2026-12-10") })).toHaveFocus();
    // Modified arrows are left to the app.
    expect(fireEvent.keyDown(screen.getByRole("grid"), { key: "ArrowLeft", metaKey: true })).toBe(true);
  });
});
