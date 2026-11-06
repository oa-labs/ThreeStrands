import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { DatePickerField, parseCalendarDate } from "./DatePickerField";
import { Modal } from "./AppChrome";
import axe from "axe-core";

function Field({ value = "2026-01-31" }: { value?: string }) {
  const [date, setDate] = useState(value);
  return <DatePickerField label="Start date" value={date} onChange={setDate} />;
}

const open = () => {
  fireEvent.click(screen.getByRole("button", { name: "Choose start date" }));
  return screen.getByRole("dialog", { name: "Choose start date" });
};

afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe("DatePickerField", () => {
  it("exposes labelled controls and an accessible calendar grid", async () => {
    render(<Field />);
    const picker = open();
    const result = await axe.run(picker, { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa", "wcag21aa"] } });
    expect(result.violations).toEqual([]);
  });
  it("opens on the selected day, moves between months, and chooses a day without submitting", () => {
    const submit = vi.fn((event) => event.preventDefault());
    render(<form onSubmit={submit}><Field /></form>);
    const picker = open();
    expect(within(picker).getByRole("grid", { name: "January 2026" })).toBeInTheDocument();
    expect(within(picker).getByRole("button", { name: "Saturday, January 31, 2026" })).toHaveFocus();
    expect(within(picker).getByRole("gridcell", { selected: true })).toHaveTextContent("31");
    fireEvent.click(within(picker).getByRole("button", { name: "Next month" }));
    expect(within(picker).getByRole("grid", { name: "February 2026" })).toBeInTheDocument();
    fireEvent.click(within(picker).getByRole("button", { name: "Saturday, February 28, 2026" }));
    expect(screen.getByLabelText("Start date")).toHaveValue("2026-02-28");
    expect(screen.getByLabelText("Start date")).toHaveFocus();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(submit).not.toHaveBeenCalled();
    open();
    fireEvent.click(screen.getByRole("button", { name: "Previous month" }));
    expect(screen.getByRole("grid", { name: "January 2026" })).toBeInTheDocument();
  });

  it("supports keyboard navigation across month and year boundaries and clamps month jumps", () => {
    render(<Field />);
    fireEvent.keyDown(screen.getByLabelText("Start date"), { key: "ArrowDown" });
    const grid = screen.getByRole("grid");
    const move = (key: string, day: string, shiftKey = false) => {
      fireEvent.keyDown(grid, { key, shiftKey });
      expect(screen.getByRole("button", { name: day })).toHaveFocus();
    };
    move("PageDown", "Saturday, February 28, 2026");
    move("ArrowRight", "Sunday, March 1, 2026");
    move("ArrowUp", "Sunday, February 22, 2026");
    move("End", "Saturday, February 28, 2026");
    move("Home", "Sunday, February 22, 2026");
    move("ArrowDown", "Sunday, March 1, 2026");
    move("ArrowLeft", "Saturday, February 28, 2026");
    move("PageUp", "Friday, February 28, 2025", true);
    fireEvent.click(document.activeElement!);
    expect(screen.getByLabelText("Start date")).toHaveValue("2025-02-28");
  });

  it("dismisses with outside clicks and focus leaving the field", () => {
    render(<><Field /><button>Elsewhere</button></>);
    open();
    fireEvent.pointerDown(screen.getByRole("button", { name: "Elsewhere" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    open();
    act(() => screen.getByRole("button", { name: "Elsewhere" }).focus());
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("takes Escape before the event dialog and returns focus to the date", () => {
    const close = vi.fn();
    render(<Modal title="New event" onClose={close}><Field /></Modal>);
    open();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Choose start date" })).not.toBeInTheDocument();
    expect(close).not.toHaveBeenCalled();
    expect(screen.getByLabelText("Start date")).toHaveFocus();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(close).toHaveBeenCalledOnce();
  });

  it("allows typed dates and rejects impossible dates without rolling into the next month", () => {
    render(<Field />);
    const input = screen.getByLabelText("Start date");
    fireEvent.change(input, { target: { value: "2024-02-29" } });
    expect(input).toBeValid();
    fireEvent.change(input, { target: { value: "2026-02-29" } });
    expect(input).toBeInvalid();
    fireEvent.change(input, { target: { value: "2026-04-31" } });
    expect(input).toBeInvalid();
    fireEvent.change(input, { target: { value: "2026-12-31" } });
    expect(input).toBeValid();
    open();
    fireEvent.click(screen.getByRole("button", { name: "Today" }));
    const now = new Date();
    expect(input).toHaveValue(`${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`);
  });
});

describe("parseCalendarDate", () => {
  it.each(["", "0000-01-01", "2026-2-01", "2026-02-30", "2026-13-01", "2026-00-01", "2026-01-00", "not a date"])("rejects %s", (value) => {
    expect(parseCalendarDate(value)).toBeNull();
  });
  it("keeps dates local through DST and year changes", () => {
    for (const value of ["2024-02-29", "2026-03-08", "2026-11-01", "2026-12-31", "2027-01-01"]) {
      const date = parseCalendarDate(value)!;
      expect([date.getFullYear(), date.getMonth() + 1, date.getDate()]).toEqual(value.split("-").map(Number));
    }
  });
});
