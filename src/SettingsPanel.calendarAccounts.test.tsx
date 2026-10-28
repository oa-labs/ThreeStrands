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
      onReorder={vi.fn()}
      onSetSelection={vi.fn()}
      onSetAvailabilitySelection={vi.fn()}
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

  it("defaults availability to primary while keeping visibility separate", async () => {
    const change = vi.fn().mockResolvedValue(undefined);
    const visible = vi.fn().mockResolvedValue(undefined);
    const calendars: CalendarOption[] = [
      { id: "primary", accountId: account.email, name: "Me", primary: true, selected: false, writable: true },
      { id: "holiday", accountId: account.email, name: "Holidays", primary: false, selected: true, writable: false },
    ];
    render(<CalendarAccountsSettings authStatus={null} accounts={[account]} calendars={calendars} calendarsLoaded calendarsError={null}
      onAdd={vi.fn()} onReconnect={vi.fn()} onRemove={vi.fn()} onRemoveEverywhere={vi.fn()} onReorder={vi.fn()}
      onSetSelection={visible} onSetAvailabilitySelection={change} />);
    const availability = screen.getByRole("group", { name: "Calendars used for availability" });
    const shown = screen.getByRole("group", { name: "Calendars shown in the sidebar" });
    expect(within(availability).getByRole("checkbox", { name: "Me (Primary)" })).toBeChecked();
    expect(within(availability).getByRole("checkbox", { name: "Holidays" })).not.toBeChecked();
    expect(within(shown).getByRole("checkbox", { name: "Holidays" })).toBeChecked();
    fireEvent.click(within(availability).getByRole("checkbox", { name: "Holidays" }));
    await waitFor(() => expect(change).toHaveBeenCalledWith(account.email, ["primary", "holiday"]));
    expect(visible).not.toHaveBeenCalled();
  });

  it("saves an explicit sidebar selection, including an empty one", async () => {
    const visible = vi.fn().mockResolvedValue(undefined);
    const calendars: CalendarOption[] = [
      { id: "primary", accountId: account.email, name: "Personal", primary: true, selected: true, writable: true },
      { id: "team", accountId: account.email, name: "Team", primary: false, selected: false, writable: true },
    ];
    render(<CalendarAccountsSettings authStatus={null} accounts={[account]} calendars={calendars} calendarsLoaded calendarsError={null}
      onAdd={vi.fn()} onReconnect={vi.fn()} onRemove={vi.fn()} onRemoveEverywhere={vi.fn()} onReorder={vi.fn()}
      onSetSelection={visible} onSetAvailabilitySelection={vi.fn()} />);
    const shown = screen.getByRole("group", { name: "Calendars shown in the sidebar" });
    const primary = within(shown).getByRole("checkbox", { name: "Personal (Primary)" });
    expect(primary).toBeChecked();
    fireEvent.click(primary);
    await waitFor(() => expect(visible).toHaveBeenCalledWith(account.email, []));
  });

  it("always includes an available holiday subscription without requiring it to be visible", () => {
    renderPicker({ calendarsLoaded: true, calendars: [{ id: "fr.fr#holiday@group.v.calendar.google.com", accountId: account.email, name: "Jours fériés", primary: false, selected: false, writable: false, availabilitySelected: false }] });
    const availability = screen.getByRole("group", { name: "Calendars used for availability" });
    expect(within(availability).getByRole("checkbox", { name: "Jours fériés (Included when available)" })).toBeChecked();
    expect(within(availability).getByRole("checkbox")).toBeDisabled();
    const shown = screen.getByRole("group", { name: "Calendars shown in the sidebar" });
    expect(within(shown).getByRole("checkbox", { name: "Jours fériés" })).not.toBeChecked();
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
    expect(within(picker).getByRole("button", { name: "Options for Team" })).toHaveFocus();
  });

  it("closes a calendar's color menu on Escape and returns focus to its options button", () => {
    const picker = renderPicker({
      calendarsLoaded: true,
      calendars: [{ id: "team", accountId: account.email, name: "Team", primary: false, selected: true } as CalendarOption],
    });
    const trigger = within(picker).getByRole("button", { name: "Options for Team" });
    fireEvent.click(trigger);
    const palette = within(picker).getByRole("menu", { name: "Color for Team" });
    within(palette).getByRole("menuitemradio", { name: "Default" }).focus();

    fireEvent.keyDown(window, { key: "Escape" });
    expect(within(picker).queryByRole("menu", { name: "Color for Team" })).not.toBeInTheDocument();
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(trigger).toHaveFocus();
    expect(localStorage.getItem(CALENDAR_COLORS_KEY)).toBeNull();
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
    if (scope === "Remove on all devices") {
      // The broadest scope is gated behind a second "are you sure?" step, so the
      // first click only reveals the confirmation and calls nothing yet.
      expect(onRemoveEverywhere).not.toHaveBeenCalled();
      const sure = screen.getByRole("group", { name: "Remove calendar account on all devices confirmation" });
      fireEvent.click(within(sure).getByRole("button", { name: "Yes, remove everywhere" }));
    }
    const selected = scope === "Disconnect this device" ? onRemove : onRemoveEverywhere;
    const other = scope === "Disconnect this device" ? onRemoveEverywhere : onRemove;
    await waitFor(() => expect(selected).toHaveBeenCalledWith(account.email));
    expect(selected).toHaveBeenCalledTimes(1);
    expect(other).not.toHaveBeenCalled();
  });
});

describe("calendar account order", () => {
  const work = { email: "work@example.com", status: "connected" } as CalendarAccount;
  const home = { email: "home@example.com", status: "connected" } as CalendarAccount;
  const side = { email: "side@example.com", status: "connected" } as CalendarAccount;

  function renderAccounts(accounts: CalendarAccount[], onReorder = vi.fn().mockResolvedValue(undefined)) {
    render(
      <CalendarAccountsSettings
        authStatus={null}
        accounts={accounts}
        calendars={[]}
        calendarsError={null}
        calendarsLoaded
        onAdd={vi.fn()}
        onReconnect={vi.fn()}
        onRemove={vi.fn()}
        onRemoveEverywhere={vi.fn()}
        onReorder={onReorder}
        onSetSelection={vi.fn()}
      onSetAvailabilitySelection={vi.fn()}
      />,
    );
    return onReorder;
  }

  it("moves an account up or down one place", async () => {
    const onReorder = renderAccounts([home, side, work]);

    fireEvent.click(screen.getByRole("button", { name: "Move work@example.com up" }));
    await waitFor(() => expect(onReorder).toHaveBeenCalledWith([home.email, work.email, side.email]));

    fireEvent.click(screen.getByRole("button", { name: "Move home@example.com down" }));
    await waitFor(() => expect(onReorder).toHaveBeenLastCalledWith([side.email, home.email, work.email]));
  });

  it("disables moves past either end", () => {
    renderAccounts([home, work]);
    expect(screen.getByRole("button", { name: "Move home@example.com up" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Move home@example.com down" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Move work@example.com up" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Move work@example.com down" })).toBeDisabled();
  });

  it("hides the order controls with a single account", () => {
    renderAccounts([work]);
    expect(screen.queryByRole("group", { name: "Order for work@example.com" })).not.toBeInTheDocument();
  });

  it("shows why a new order could not be saved", async () => {
    renderAccounts([home, work], vi.fn().mockRejectedValue(new Error("database locked")));
    fireEvent.click(screen.getByRole("button", { name: "Move work@example.com up" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("database locked");
  });
});
