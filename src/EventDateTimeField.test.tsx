import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
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
    expect(time).toHaveAttribute("type", "time");
    fireEvent.click(time);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    fireEvent.change(time, { target: { value: "16:45" } });
    expect(screen.getByLabelText("Start date")).toHaveValue("2026-11-12");
    fireEvent.click(screen.getByRole("button", { name: "Choose start date" }));
    fireEvent.click(screen.getByRole("button", { name: "Friday, November 13, 2026" }));
    expect(screen.getByLabelText("Start date")).toHaveValue("2026-11-13");
    expect(time).toHaveValue("16:45");
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
