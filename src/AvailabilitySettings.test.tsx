import { fireEvent, render, screen, within } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it } from "vitest";
import { AvailabilitySettings, makeDefaultAvailabilityProfile, nextWorkingWindow, windowsOverlap } from "./AvailabilitySettings";
import type { Account, AvailabilityPreferences, AvailabilityProfiles, AvailabilityWindow } from "./domain";
import { DEFAULT_AVAILABILITY_PROFILES, MAX_AVAILABILITY_PROFILES, MAX_WORKING_WINDOWS_PER_DAY } from "./settings";
import { settingsAccount } from "./test/settingsHarness";

const basePreferences: AvailabilityPreferences = {
  timeZone: "America/New_York",
  workingWindows: [{ weekday: 1, start: "09:00", end: "17:00" }],
  defaultDurationMinutes: 30,
  slotIncrementMinutes: 15,
  driveTimeBufferMinutes: 0,
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

const workAccount: Account = { ...settingsAccount, email: "work@example.com", nickname: "Work" };
const homeAccount: Account = { ...settingsAccount, email: "home@example.com", nickname: null };
const churchHours = [window(2, "18:30", "21:00"), window(6, "08:00", "12:00")];
const withChurch: AvailabilityProfiles = {
  defaultProfile: { id: "default", name: "Work" },
  otherProfiles: [{ id: "church", name: "Church", workingWindows: churchHours, defaultDurationMinutes: 60, driveTimeBufferMinutes: 15 }],
  accountDefaults: { "home@example.com": "church" },
};

function renderWithProfiles(initialProfiles: AvailabilityProfiles = withChurch, accounts: Account[] = [workAccount, homeAccount]) {
  const latest = { preferences: basePreferences, profiles: initialProfiles };
  function Harness() {
    const [preferences, setPreferences] = useState(basePreferences);
    const [profiles, setProfiles] = useState(initialProfiles);
    return (
      <AvailabilitySettings
        preferences={preferences}
        onChange={(value) => {
          latest.preferences = value;
          setPreferences(value);
        }}
        profiles={profiles}
        onProfilesChange={(value, nextPreferences) => {
          latest.profiles = value;
          setProfiles(value);
          if (nextPreferences) {
            latest.preferences = nextPreferences;
            setPreferences(nextPreferences);
          }
        }}
        accounts={accounts}
      />
    );
  }
  render(<Harness />);
  return latest;
}

describe("AvailabilitySettings working hours", () => {
  it("sets and disables the drive time buffer at the bottom of the panel", () => {
    const latest = renderSettings();
    const buffer = screen.getByRole("combobox", { name: "Drive Time Buffer" });
    const fields = screen.getAllByRole("combobox");
    expect(fields.at(-1)).toBe(buffer);
    expect(buffer).toHaveValue("0");
    fireEvent.change(buffer, { target: { value: "30" } });
    expect(latest.current).toEqual({ ...basePreferences, driveTimeBufferMinutes: 30 });
    fireEvent.change(buffer, { target: { value: "0" } });
    expect(latest.current).toEqual(basePreferences);
  });

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

describe("AvailabilitySettings profiles", () => {
  it("shows one set of hours until a second profile exists", () => {
    renderWithProfiles(DEFAULT_AVAILABILITY_PROFILES);
    expect(screen.queryByRole("group", { name: "Profiles" })).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: /work@example.com/ })).not.toBeInTheDocument();
    expect(screen.getByLabelText("Availability Timezone")).not.toHaveAccessibleDescription();
  });

  it("adds a profile from the default hours and edits it without changing the default", () => {
    const latest = renderWithProfiles(DEFAULT_AVAILABILITY_PROFILES);
    fireEvent.click(screen.getByRole("button", { name: "Add Profile" }));

    const [added] = latest.profiles.otherProfiles;
    expect(added).toMatchObject({ name: "New Profile", workingWindows: basePreferences.workingWindows, defaultDurationMinutes: 30 });
    expect(within(screen.getByRole("group", { name: "Profiles" })).getByRole("button", { name: "New Profile" })).toHaveAttribute("aria-pressed", "true");

    fireEvent.change(screen.getByLabelText("Monday End"), { target: { value: "12:00" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Default Duration" }), { target: { value: "60" } });
    expect(latest.profiles.otherProfiles[0]).toMatchObject({ workingWindows: [window(1, "09:00", "12:00")], defaultDurationMinutes: 60 });
    expect(latest.preferences).toEqual(basePreferences);
  });

  it("edits the shared timezone and slot increment for every profile and says so", () => {
    const latest = renderWithProfiles();
    fireEvent.click(screen.getByRole("button", { name: "Church" }));
    const timeZone = screen.getByLabelText("Availability Timezone");
    expect(timeZone).toHaveAccessibleDescription();
    expect(screen.getByRole("combobox", { name: "Slot Increment" })).toHaveAccessibleDescription();
    expect(screen.getByRole("group", { name: "Profiles" })).toHaveAccessibleDescription();

    fireEvent.change(screen.getByRole("combobox", { name: "Slot Increment" }), { target: { value: "30" } });
    expect(latest.preferences.slotIncrementMinutes).toBe(30);
    expect(latest.profiles).toEqual(withChurch);
  });

  it("renames a profile, refusing a name another profile has", () => {
    const latest = renderWithProfiles();
    fireEvent.click(screen.getByRole("button", { name: "Church" }));
    const name = screen.getByRole("textbox", { name: "Profile Name" });

    fireEvent.change(name, { target: { value: "work " } });
    fireEvent.blur(name);
    expect(screen.getByRole("alert")).toBeInTheDocument();
    expect(latest.profiles).toEqual(withChurch);

    fireEvent.change(name, { target: { value: " Elders " } });
    fireEvent.keyDown(name, { key: "Enter" });
    expect(latest.profiles.otherProfiles[0].name).toBe("Elders");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("chooses each account's profile, and the default profile stores nothing", () => {
    const latest = renderWithProfiles();
    const work = screen.getByRole("combobox", { name: "Work (work@example.com)" });
    const home = screen.getByRole("combobox", { name: "home@example.com" });
    expect(work).toHaveValue("default");
    expect(home).toHaveValue("church");

    fireEvent.change(work, { target: { value: "church" } });
    expect(latest.profiles.accountDefaults).toEqual({ "home@example.com": "church", "work@example.com": "church" });
    fireEvent.change(home, { target: { value: "default" } });
    expect(latest.profiles.accountDefaults).toEqual({ "work@example.com": "church" });
  });

  it("deletes a profile after naming the accounts that move to the default profile", () => {
    const latest = renderWithProfiles();
    fireEvent.click(screen.getByRole("button", { name: "Church" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete Profile" }));
    const confirm = screen.getByRole("group", { name: "Delete Church confirmation" });
    expect(confirm).toHaveTextContent("home@example.com will use Work.");

    fireEvent.click(within(confirm).getByRole("button", { name: "Delete Profile" }));
    expect(latest.profiles).toEqual({ ...withChurch, otherProfiles: [], accountDefaults: {} });
    expect(screen.queryByRole("group", { name: "Profiles" })).not.toBeInTheDocument();
  });

  it("makes another profile the default, which every account without its own profile then uses", () => {
    const latest = renderWithProfiles();
    fireEvent.click(screen.getByRole("button", { name: "Church" }));
    fireEvent.click(screen.getByRole("button", { name: "Make Default" }));

    expect(latest.preferences).toEqual({ ...basePreferences, workingWindows: churchHours, defaultDurationMinutes: 60, driveTimeBufferMinutes: 15 });
    expect(latest.profiles.defaultProfile).toEqual({ id: "church", name: "Church" });
    expect(latest.profiles.otherProfiles).toEqual([{
      id: "default", name: "Work",
      workingWindows: basePreferences.workingWindows, defaultDurationMinutes: 30, driveTimeBufferMinutes: 0,
    }]);
    // Church is now the default, so neither account needs an entry of its own.
    expect(latest.profiles.accountDefaults).toEqual({});
    expect(screen.getByRole("combobox", { name: "home@example.com" })).toHaveValue("church");
    expect(screen.getByRole("combobox", { name: "Work (work@example.com)" })).toHaveValue("church");
  });

  it("stops adding profiles at the limit", () => {
    const template = withChurch.otherProfiles[0];
    renderWithProfiles({
      ...withChurch,
      otherProfiles: Array.from({ length: MAX_AVAILABILITY_PROFILES - 2 }, (_, index) => ({ ...template, id: `p${index}`, name: `Profile ${index}` })),
      accountDefaults: {},
    });
    fireEvent.click(screen.getByRole("button", { name: "Add Profile" }));
    expect(within(screen.getByRole("group", { name: "Profiles" })).getAllByRole("button")).toHaveLength(MAX_AVAILABILITY_PROFILES);
    expect(screen.getByRole("button", { name: "Add Profile" })).toBeDisabled();
  });
});

describe("makeDefaultAvailabilityProfile", () => {
  it("keeps an account that chose the old default on its hours", () => {
    const profiles = { ...withChurch, otherProfiles: [...withChurch.otherProfiles, { ...withChurch.otherProfiles[0], id: "board", name: "Board" }] };
    const next = makeDefaultAvailabilityProfile(basePreferences, profiles, "board");
    // home@example.com chose Church, which keeps its id and hours.
    expect(next.profiles.accountDefaults).toEqual({ "home@example.com": "church" });
    expect(next.profiles.otherProfiles.map((profile) => profile.id)).toEqual(["church", "default"]);
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
