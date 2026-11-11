import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AvailabilityAccountDefaultsSettings, MeetingTypeProfilesSettings } from "./AvailabilityChoiceSettings";
import type { Account, AvailabilityProfiles } from "./domain";
import { DEFAULT_AVAILABILITY_PROFILES } from "./settings";
import { SettingsHarness, settingsAccount } from "./test/settingsHarness";

const withLunch: AvailabilityProfiles = {
  defaultProfile: { id: "default", name: "Meetings" },
  otherProfiles: [{ id: "lunch", name: "Lunch", workingWindows: [{ weekday: 1, start: "12:00", end: "13:30" }], defaultDurationMinutes: 60, driveTimeBufferMinutes: 0 }],
  accountDefaults: {},
  meetingTypeProfiles: { lunch: "lunch" },
};
const named: Account = { ...settingsAccount, email: "work@example.com", nickname: "Work" };
const displayNamed: Account = { ...settingsAccount, email: "joel@example.com", displayName: "Joel", nickname: null };
const unnamed: Account = { ...settingsAccount, email: "home@example.com", nickname: null, displayName: null };

describe("AvailabilityAccountDefaultsSettings", () => {
  it("names each account first, with its address beneath, and says what the default resolves to", () => {
    render(<AvailabilityAccountDefaultsSettings profiles={withLunch} onProfilesChange={vi.fn()} accounts={[named, displayNamed, unnamed]} />);
    expect(screen.getByRole("combobox", { name: "Work work@example.com" })).toHaveDisplayValue("Use default · Meetings");
    expect(screen.getByRole("combobox", { name: "Joel joel@example.com" })).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "home@example.com" })).toBeInTheDocument();
  });

  it("points to Profiles when no other profile exists or no account is connected", () => {
    const onOpenProfiles = vi.fn();
    render(<AvailabilityAccountDefaultsSettings profiles={DEFAULT_AVAILABILITY_PROFILES} onProfilesChange={vi.fn()} accounts={[]} onOpenProfiles={onOpenProfiles} />);
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Open Profiles" }));
    expect(onOpenProfiles).toHaveBeenCalled();
  });
});

describe("MeetingTypeProfilesSettings", () => {
  it("offers Edit Profile only for a meeting type with a profile", () => {
    const onEditProfile = vi.fn();
    render(<MeetingTypeProfilesSettings profiles={withLunch} onProfilesChange={vi.fn()} onEditProfile={onEditProfile} />);
    expect(screen.getByRole("combobox", { name: "Lunch" })).toHaveValue("lunch");
    expect(screen.getByRole("combobox", { name: "Coffee" })).toHaveValue("");
    expect(screen.getAllByRole("button", { name: /^Edit Profile/ })).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Edit Profile Lunch for Lunch" }));
    expect(onEditProfile).toHaveBeenCalledWith("lunch");
  });
});

describe("Availability settings pages", () => {
  it("lists Profiles, Account Defaults, and Meeting Types under Availability", () => {
    render(<SettingsHarness initialSection="availability" />);
    const nav = screen.getByRole("navigation", { name: "Settings sections" });
    const group = within(nav).getByText("Availability").parentElement!;
    expect(within(group).getAllByRole("button").map((button) => button.textContent)).toEqual(["Profiles", "Account Defaults", "Meeting Types"]);
    expect(screen.getByRole("region", { name: "Profiles" })).toBeInTheDocument();

    fireEvent.click(within(nav).getByRole("button", { name: "Account Defaults" }));
    expect(screen.getByRole("region", { name: "Account Defaults" })).toBeInTheDocument();
    fireEvent.click(within(nav).getByRole("button", { name: "Meeting Types" }));
    expect(screen.getByRole("region", { name: "Meeting Types" })).toBeInTheDocument();
  });

  it("opens the meeting type's profile on Profiles from Edit Profile", () => {
    render(<SettingsHarness initialSection="availability" />);
    fireEvent.click(screen.getByRole("button", { name: "Add Profile" }));
    const nav = screen.getByRole("navigation", { name: "Settings sections" });
    // Leave the new profile selected elsewhere, so Edit Profile has to change it.
    fireEvent.click(within(screen.getByRole("group", { name: "Profiles" })).getByRole("button", { name: "Default" }));
    fireEvent.click(within(nav).getByRole("button", { name: "Meeting Types" }));
    fireEvent.change(screen.getByRole("combobox", { name: "Coffee" }), { target: { value: within(screen.getByRole("combobox", { name: "Coffee" })).getByRole("option", { name: "New Profile" }).getAttribute("value") } });
    fireEvent.click(screen.getByRole("button", { name: "Edit Profile New Profile for Coffee" }));

    expect(screen.getByRole("region", { name: "Profiles" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "New Profile" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("region", { name: "New Profile" })).toBeInTheDocument();
  });
});
