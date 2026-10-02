import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { CalendarAccount, CalendarOption } from "./domain";
import { CalendarAccountsSettings } from "./SettingsPanel";

afterEach(cleanup);

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
});
