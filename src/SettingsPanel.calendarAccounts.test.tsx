import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { CALENDAR_COLORS_KEY, resetCalendarColorsForTests } from "./calendarColors";
import type { CalendarAccount, CalendarOption } from "./domain";
import { CalendarAccountsSettings } from "./CalendarAccountsSettings";

afterEach(() => {
  cleanup();
  localStorage.clear();
  resetCalendarColorsForTests();
});

const account: CalendarAccount = { email: "me@example.com", status: "connected" } as CalendarAccount;

function renderPicker(props: {
  calendars?: CalendarOption[];
  calendarsError?: string | null;
  calendarsLoaded: boolean;
  onRemove?: (email: string) => Promise<void>;
  onRemoveEverywhere?: (email: string) => Promise<void>;
}) {
  render(
    <CalendarAccountsSettings
      authStatus={null}
      accounts={[account]}
      calendars={props.calendars ?? []}
      calendarsError={props.calendarsError ?? null}
      calendarsLoaded={props.calendarsLoaded}
      onAdd={vi.fn()}
      onReconnect={vi.fn()}
      onRemove={props.onRemove ?? vi.fn()}
      onRemoveEverywhere={props.onRemoveEverywhere ?? vi.fn()}
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
    // Sixteen colors plus Default.
    expect(within(palette).getAllByRole("menuitemradio")).toHaveLength(17);
    expect(within(palette).getByRole("menuitemradio", { name: "Default" })).toHaveAttribute("aria-checked", "true");
    fireEvent.click(within(palette).getByRole("menuitemradio", { name: "Purple" }));

    expect(JSON.parse(localStorage.getItem(CALENDAR_COLORS_KEY)!)).toEqual({ [account.email]: { team: "purple" } });
    const teamRow = within(picker).getByRole("checkbox", { name: "Team" }).closest(".calendar-color-row") as HTMLElement;
    expect(teamRow.style.getPropertyValue("--calendar-color")).toBe("#8a55c9");
    const meRow = within(picker).getByRole("checkbox", { name: "Me (Primary)" }).closest(".calendar-color-row") as HTMLElement;
    expect(meRow.style.getPropertyValue("--calendar-color")).toBe("");
  });

  it.each(["Disconnect this device", "Remove on all devices"])("confirms the calendar removal scope: %s", async (scope) => {
    const onRemove = vi.fn().mockResolvedValue(undefined);
    const onRemoveEverywhere = vi.fn().mockResolvedValue(undefined);
    renderPicker({ calendarsLoaded: true, onRemove, onRemoveEverywhere });

    fireEvent.click(screen.getByRole("button", { name: "Disconnect…" }));
    const confirmation = screen.getByRole("group", { name: "Disconnect calendar account confirmation" });
    expect(onRemove).not.toHaveBeenCalled();
    expect(onRemoveEverywhere).not.toHaveBeenCalled();
    fireEvent.click(within(confirmation).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("group", { name: "Disconnect calendar account confirmation" })).not.toBeInTheDocument();
    expect(onRemove).not.toHaveBeenCalled();
    expect(onRemoveEverywhere).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Disconnect…" }));
    fireEvent.click(within(screen.getByRole("group", { name: "Disconnect calendar account confirmation" })).getByRole("button", { name: scope }));
    const selected = scope === "Disconnect this device" ? onRemove : onRemoveEverywhere;
    const other = scope === "Disconnect this device" ? onRemoveEverywhere : onRemove;
    await waitFor(() => expect(selected).toHaveBeenCalledWith(account.email));
    expect(selected).toHaveBeenCalledTimes(1);
    expect(other).not.toHaveBeenCalled();
  });
});
