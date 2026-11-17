import axe from "axe-core";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { formatEventTime, parseEventTime } from "./TimePickerField";
import { EventDateTimeField } from "./EventDateTimeField";

function Field({ allDay = false, initial = "2026-11-12T14:00" }) {
  const [value, setValue] = useState(initial);
  return <EventDateTimeField label="Starts" value={value} allDay={allDay} onChange={setValue} />;
}

afterEach(cleanup);

describe("EventDateTimeField", () => {
  it("edits time without opening a calendar, and changes date without changing time", () => {
    render(<Field />);
    const time = screen.getByLabelText("Start time");
    expect(time).toHaveAttribute("role", "combobox");
    fireEvent.click(time);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    fireEvent.change(time, { target: { value: "16:45" } });
    expect(screen.getByLabelText("Start date")).toHaveValue("2026-11-12");
    fireEvent.click(screen.getByRole("button", { name: "Choose start date" }));
    fireEvent.click(screen.getByRole("button", { name: "Friday, November 13, 2026" }));
    expect(screen.getByLabelText("Start date")).toHaveValue("2026-11-13");
    expect(time).toHaveValue("16:45");
    fireEvent.blur(time);
    expect(time).toHaveValue("4:45 PM");
  });

  it("shows times with AM or PM", () => {
    render(<Field initial="2026-11-12T10:00" />);
    expect(screen.getByLabelText("Start time")).toHaveValue("10:00 AM");
    cleanup();
    render(<Field initial="2026-11-12T00:30" />);
    expect(screen.getByLabelText("Start time")).toHaveValue("12:30 AM");
  });

  it("offers quarter-hour choices, supports keyboard selection, and keeps exact typed times", () => {
    render(<Field />);
    const input = screen.getByRole("combobox", { name: "Start time" });
    fireEvent.focus(input);
    expect(screen.getByRole("listbox", { name: "Start time suggestions" })).toBeInTheDocument();
    fireEvent.mouseDown(screen.getByRole("option", { name: "8:15 AM" }));
    expect(input).toHaveValue("8:15 AM");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    fireEvent.change(input, { target: { value: "8:50am" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(input).toHaveValue("8:50 AM");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(input).toHaveValue("9:00 AM");
    expect(screen.getByLabelText("Start date")).toHaveValue("2026-11-12");
  });

  it("dismisses suggestions with Escape or blur and validates invalid input", () => {
    render(<Field />);
    const input = screen.getByRole("combobox", { name: "Start time" });
    fireEvent.focus(input);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    fireEvent.change(input, { target: { value: "25:00" } });
    expect(input).toBeInvalid();
    fireEvent.blur(input);
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(input).toHaveValue("25:00");
    fireEvent.change(input, { target: { value: "12am" } });
    fireEvent.blur(input);
    expect(input).toHaveValue("12:00 AM");
    expect(input).toBeValid();
  });

  it.each([["8am", "08:00"], ["8:50 pm", "20:50"], ["23:59", "23:59"], ["12pm", "12:00"], ["13pm", null], ["8:99", null], ["", null]])("parses %s", (text, expected) => {
    expect(parseEventTime(text)).toBe(expected);
  });

  it.each([["00:00", "12:00 AM"], ["08:05", "8:05 AM"], ["12:00", "12:00 PM"], ["23:59", "11:59 PM"], ["", ""]])("formats %s", (value, expected) => {
    expect(formatEventTime(value)).toBe(expected);
    if (value) expect(parseEventTime(expected)).toBe(value);
  });

  it("preserves an exact off-grid time after reopening, including the last minute of the day", () => {
    render(<Field initial="2026-11-12T23:59" />);
    const input = screen.getByLabelText("Start time");
    fireEvent.focus(input);
    fireEvent.keyDown(input, { key: "Enter" });
    expect(input).toHaveValue("11:59 PM");
  });

  it("cannot choose another time while the form is disabled for saving", () => {
    const view = render(<fieldset><Field /></fieldset>);
    fireEvent.focus(screen.getByLabelText("Start time"));
    view.rerender(<fieldset disabled><Field /></fieldset>);
    fireEvent.mouseDown(screen.getByRole("option", { name: "8:15 AM" }));
    expect(screen.getByLabelText("Start time")).toHaveValue("2:00 PM");
  });

  it("exposes an accessible editable combobox and suggestions", async () => {
    const { container } = render(<Field />);
    fireEvent.focus(screen.getByLabelText("Start time"));
    const result = await axe.run(container, { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa", "wcag21aa"] } });
    expect(result.violations.filter((violation) => violation.impact === "serious" || violation.impact === "critical")).toEqual([]);
  });

  it("keeps unspecified times blank when choosing a date", () => {
    render(<Field initial="2026-11-12" />);
    fireEvent.click(screen.getByRole("button", { name: "Choose start date" }));
    fireEvent.click(screen.getByRole("button", { name: "Friday, November 13, 2026" }));
    expect(screen.getByLabelText("Start time")).toHaveValue("");
    expect(screen.getByLabelText("Start time")).toBeRequired();
  });

  it("offers only a date for all-day events", () => {
    render(<Field initial="2026-11-12" allDay />);
    expect(screen.queryByLabelText("Start time")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Choose start date" }));
    fireEvent.click(screen.getByRole("button", { name: "Friday, November 13, 2026" }));
    expect(screen.getByLabelText("Start date")).toHaveValue("2026-11-13");
  });
});
