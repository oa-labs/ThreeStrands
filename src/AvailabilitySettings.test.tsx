import { fireEvent, render, screen, within } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it } from "vitest";
import { AvailabilitySettings, nextWorkingWindow, windowsOverlap } from "./AvailabilitySettings";
import type { AvailabilityPreferences, AvailabilityWindow } from "./domain";
import { MAX_WORKING_WINDOWS_PER_DAY } from "./settings";

const basePreferences: AvailabilityPreferences = {
  timeZone: "America/New_York",
  workingWindows: [{ weekday: 1, start: "09:00", end: "17:00" }],
  defaultDurationMinutes: 30,
  slotIncrementMinutes: 15,
};

function renderSettings(initial: AvailabilityPreferences = basePreferences) {
  const latest: { current: AvailabilityPreferences } = { current: initial };
  function Harness() {
    const [preferences, setPreferences] = useState(initial);
    return (
      <AvailabilitySettings
        preferences={preferences}
        onChange={(value) => {
          latest.current = value;
          setPreferences(value);
        }}
      />
    );
  }
  render(<Harness />);
  return latest;
}

const window = (weekday: number, start: string, end: string): AvailabilityWindow => ({ weekday, start, end });

describe("AvailabilitySettings working hours", () => {
  it("splits a day around lunch with a second slot", () => {
    const latest = renderSettings();
    fireEvent.change(screen.getByLabelText("Monday End"), { target: { value: "12:00" } });
    fireEvent.click(screen.getByRole("button", { name: "Add Monday slot" }));
    expect(screen.getByLabelText("Monday Start 2")).toHaveValue("13:00");
    fireEvent.change(screen.getByLabelText("Monday End 2"), { target: { value: "17:00" } });
    expect(latest.current.workingWindows).toEqual([window(1, "09:00", "12:00"), window(1, "13:00", "17:00")]);
  });

  it("removes one slot and keeps the others", () => {
    const latest = renderSettings({
      ...basePreferences,
      workingWindows: [window(1, "09:00", "12:00"), window(1, "13:00", "17:00"), window(2, "09:00", "17:00")],
    });
    expect(screen.queryByRole("button", { name: "Remove Tuesday slot 1" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Remove Monday slot 1" }));
    expect(latest.current.workingWindows).toEqual([window(1, "13:00", "17:00"), window(2, "09:00", "17:00")]);
    expect(screen.getByLabelText("Monday Start")).toHaveValue("13:00");
  });

  it("unchecking a day clears all of its slots", () => {
    const latest = renderSettings({
      ...basePreferences,
      workingWindows: [window(1, "09:00", "12:00"), window(1, "13:00", "17:00"), window(2, "09:00", "17:00")],
    });
    fireEvent.click(screen.getByRole("checkbox", { name: "Monday" }));
    expect(latest.current.workingWindows).toEqual([window(2, "09:00", "17:00")]);
    expect(screen.queryByRole("group", { name: "Monday Hours" })).not.toBeInTheDocument();
  });

  it("limits each day to the maximum number of slots", () => {
    renderSettings();
    for (let count = 1; count < MAX_WORKING_WINDOWS_PER_DAY; count += 1) {
      fireEvent.click(screen.getByRole("button", { name: "Add Monday slot" }));
    }
    const slots = within(screen.getByRole("group", { name: "Monday Hours" })).getAllByLabelText(/^Monday Start/);
    expect(slots).toHaveLength(MAX_WORKING_WINDOWS_PER_DAY);
    expect(screen.getByRole("button", { name: "Add Monday slot" })).toBeDisabled();
  });

  it("copies every Monday slot to the other weekdays and keeps weekends", () => {
    const latest = renderSettings({
      ...basePreferences,
      workingWindows: [window(0, "10:00", "11:00"), window(1, "09:00", "12:00"), window(1, "13:00", "17:00"), window(3, "08:00", "09:00")],
    });
    fireEvent.click(screen.getByRole("button", { name: "Copy Monday to weekdays" }));
    expect(latest.current.workingWindows).toEqual([
      window(0, "10:00", "11:00"),
      ...[1, 2, 3, 4, 5].flatMap((weekday) => [window(weekday, "09:00", "12:00"), window(weekday, "13:00", "17:00")]),
    ]);
  });

  it("notes overlapping slots", () => {
    renderSettings({ ...basePreferences, workingWindows: [window(1, "09:00", "12:00"), window(1, "11:00", "13:00")] });
    expect(within(screen.getByRole("group", { name: "Monday Hours" })).getByText("Overlapping slots count as one.")).toBeInTheDocument();
  });
});

describe("nextWorkingWindow", () => {
  it("starts a full day when the day is empty", () => {
    expect(nextWorkingWindow(2, [])).toEqual(window(2, "09:00", "17:00"));
  });

  it("starts an hour after the latest slot ends", () => {
    expect(nextWorkingWindow(1, [window(1, "13:00", "17:00"), window(1, "09:00", "12:00")])).toEqual(window(1, "18:00", "19:00"));
  });

  it("returns null when the slot would run past midnight", () => {
    expect(nextWorkingWindow(1, [window(1, "20:00", "21:59")])).toEqual(window(1, "22:59", "23:59"));
    expect(nextWorkingWindow(1, [window(1, "20:00", "22:00")])).toBeNull();
  });
});

describe("windowsOverlap", () => {
  it("treats touching slots as separate and overlapping slots as overlapping", () => {
    expect(windowsOverlap([window(1, "12:00", "13:00"), window(1, "09:00", "12:00")])).toBe(false);
    expect(windowsOverlap([window(1, "11:00", "13:00"), window(1, "09:00", "12:00")])).toBe(true);
  });
});
