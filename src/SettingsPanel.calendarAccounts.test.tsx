import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { CALENDAR_COLORS_KEY, resetCalendarColorsForTests } from "./calendarColors";
import type { CalendarAccount, CalendarOption } from "./domain";
import { CalendarAccountsSettings } from "./SettingsPanel";

afterEach(() => {
  cleanup();
  localStorage.clear();
  resetCalendarColorsForTests();
});

const account: CalendarAccount = { email: "me@example.com", status: "connected" } as CalendarAccount;

function renderPicker(props: { calendars?: CalendarOption[]; calendarsError?: string | null; calendarsLoaded: boolean }) {
  render(
    <CalendarAccountsSettings
      authStatus={null}
      accounts={[account]}
      calendars={props.calendars ?? []}
      calendarsError={props.calendarsError ?? null}
      calendarsLoaded={props.calendarsLoaded}
      onAdd={vi.fn()}
      onReconnect={vi.fn()}
      onRemove={vi.fn()}
      onRemoveEverywhere={vi.fn()}
      onSetSelection={vi.fn()}
    />,
  );
  return screen.getByRole("group", { name: "Calendars shown in the sidebar" });
}

describe("calendar account picker", () => {
  it("shows loading only until the calendar list first arrives", () => {
    expect(renderPicker({ calendarsLoaded: false })).toHaveTextContent("Loading calendars…");
  });

  it("says when a loaded account has no calendars instead of loading forever", () => {
    const picker = renderPicker({ calendarsLoaded: true });
    expect(picker).toHaveTextContent("No calendars found for this account.");
    expect(picker).not.toHaveTextContent("Loading");
  });

  it("says the list could not be loaded when fetching calendars failed", () => {
    const picker = renderPicker({ calendarsLoaded: false, calendarsError: "calendar offline" });
    expect(picker).toHaveTextContent("Calendars couldn’t be loaded.");
    expect(screen.getByRole("alert")).toHaveTextContent("calendar offline");
  });

  it("lists the account's calendars once loaded", () => {
    const picker = renderPicker({
      calendarsLoaded: true,
      calendars: [{ id: "primary", accountId: account.email, name: "Me", primary: true, selected: true } as CalendarOption],
    });
    expect(within(picker).getByRole("checkbox", { name: "Me (Primary)" })).toBeChecked();
  });

  it("picks a calendar's meeting color from its options menu", () => {
    const picker = renderPicker({
      calendarsLoaded: true,
      calendars: [
        { id: "primary", accountId: account.email, name: "Me", primary: true, selected: true } as CalendarOption,
        { id: "team", accountId: account.email, name: "Team", primary: false, selected: false } as CalendarOption,
      ],
    });
    fireEvent.click(within(picker).getByRole("button", { name: "Options for Team" }));
    const palette = within(picker).getByRole("menu", { name: "Color for Team" });
    expect(within(palette).getAllByRole("menuitemradio")).toHaveLength(16);
    fireEvent.click(within(palette).getByRole("menuitemradio", { name: "Purple" }));

    expect(JSON.parse(localStorage.getItem(CALENDAR_COLORS_KEY)!)).toEqual({ [account.email]: { team: "purple" } });
    const teamRow = within(picker).getByRole("checkbox", { name: "Team" }).closest(".calendar-color-row") as HTMLElement;
    expect(teamRow.style.getPropertyValue("--calendar-color")).toBe("#8a55c9");
    const meRow = within(picker).getByRole("checkbox", { name: "Me (Primary)" }).closest(".calendar-color-row") as HTMLElement;
    expect(meRow.style.getPropertyValue("--calendar-color")).toBe("");
  });
});
