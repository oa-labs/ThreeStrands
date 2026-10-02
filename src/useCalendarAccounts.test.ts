import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { CalendarAccount, CalendarOption } from "./domain";
import { useCalendarAccounts } from "./useCalendarAccounts";
import { clearScheduleCache, readScheduleCache, refreshScheduleCache } from "./calendarScheduleCache";

const work: CalendarAccount = { email: "work@example.com", connectedAt: "2026-09-01T00:00:00Z", status: "connected" };
const home: CalendarAccount = { email: "home@example.com", connectedAt: "2026-09-01T00:00:00Z", status: "connected" };

function calendar(id: string, accountId: string, selected = true): CalendarOption {
  return { id, accountId, name: id, primary: false, selected, writable: true };
}

describe("useCalendarAccounts", () => {
  afterEach(() => {
    clearScheduleCache();
    vi.restoreAllMocks();
  });

  const request = { timeMin: "2026-09-20T00:00:00Z", timeMax: "2026-09-27T00:00:00Z", timeZone: "UTC" };
  const seedCache = () => refreshScheduleCache(request, async () => ({ events: [], errors: [] }));

  async function renderWithCalendars(accounts: CalendarAccount[], onLastAccountRemoved = vi.fn()) {
    vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue(accounts);
    vi.spyOn(mailClient, "listCalendarOptions").mockResolvedValue([
      calendar("work-main", work.email),
      calendar("home-main", home.email),
    ]);
    const hook = renderHook(() => useCalendarAccounts({ onLastAccountRemoved }));
    await waitFor(() => expect(hook.result.current.accounts).toEqual(accounts));
    await act(async () => { await hook.result.current.refreshCalendars(); });
    return { ...hook, onLastAccountRemoved };
  }

  it("drops a removed account's calendars without signalling while others remain", async () => {
    const { result, onLastAccountRemoved } = await renderWithCalendars([work, home]);
    await seedCache();
    vi.spyOn(mailClient, "removeCalendarAccount").mockResolvedValue();
    vi.mocked(mailClient.listCalendarAccounts).mockResolvedValue([home]);

    await act(async () => { await result.current.remove(work.email); });

    expect(result.current.accounts).toEqual([home]);
    expect(result.current.calendars.map((option) => option.id)).toEqual(["home-main"]);
    expect(onLastAccountRemoved).not.toHaveBeenCalled();
    expect(readScheduleCache(request)).toBeUndefined();
  });

  it("signals once the last calendar account is removed", async () => {
    const { result, onLastAccountRemoved } = await renderWithCalendars([work]);
    vi.spyOn(mailClient, "removeCalendarAccount").mockResolvedValue();
    vi.mocked(mailClient.listCalendarAccounts).mockResolvedValue([]);

    await act(async () => { await result.current.remove(work.email); });

    expect(result.current.accounts).toEqual([]);
    expect(onLastAccountRemoved).toHaveBeenCalledTimes(1);
  });

  it("replaces only the updated account's calendars when selection changes", async () => {
    const { result } = await renderWithCalendars([work, home]);
    await seedCache();
    vi.spyOn(mailClient, "setCalendarSelection").mockResolvedValue([calendar("work-main", work.email, false)]);

    await act(async () => { await result.current.setSelection(work.email, []); });

    expect(mailClient.setCalendarSelection).toHaveBeenCalledWith(work.email, []);
    expect(readScheduleCache(request)).toBeUndefined();
    expect(result.current.calendars).toEqual([
      calendar("home-main", home.email),
      calendar("work-main", work.email, false),
    ]);
  });

  it("keeps the cache on unchanged refreshes and clears it for externally changed selections", async () => {
    const { result } = await renderWithCalendars([work]);
    await seedCache();
    await act(async () => {
      await result.current.refreshAccounts();
      await result.current.refreshCalendars();
    });
    expect(readScheduleCache(request)).toBeDefined();
    vi.mocked(mailClient.listCalendarOptions).mockResolvedValue([calendar("work-main", work.email, false)]);
    await act(async () => { await result.current.refreshCalendars(); });
    expect(readScheduleCache(request)).toBeUndefined();
  });

  it("records calendar listing failures and clears them after a successful refresh", async () => {
    const { result } = await renderWithCalendars([work]);
    vi.mocked(mailClient.listCalendarOptions).mockRejectedValueOnce(new Error("calendar offline"));

    await act(async () => {
      await expect(result.current.refreshCalendars()).rejects.toThrow("calendar offline");
    });
    expect(result.current.calendarsError).toBe("calendar offline");

    await act(async () => { await result.current.refreshCalendars(); });
    expect(result.current.calendarsError).toBeNull();    expect(result.current.calendarsLoaded).toBe(true);
  });

  it("reports calendars as loaded only after the first successful listing", async () => {
    vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([work]);
    vi.spyOn(mailClient, "listCalendarOptions").mockRejectedValueOnce(new Error("calendar offline"));
    const { result } = renderHook(() => useCalendarAccounts());
    expect(result.current.calendarsLoaded).toBe(false);

    await act(async () => {
      await expect(result.current.refreshCalendars()).rejects.toThrow("calendar offline");
    });
    expect(result.current.calendarsLoaded).toBe(false);

    vi.mocked(mailClient.listCalendarOptions).mockResolvedValueOnce([]);
    await act(async () => { await result.current.refreshCalendars(); });
    expect(result.current.calendarsLoaded).toBe(true);
  });
});
