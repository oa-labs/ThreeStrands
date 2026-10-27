import { CALENDAR_AVAILABILITY_CHANGED } from "./calendarAvailability";
import { useCallback, useEffect, useRef, useState } from "react";
import { clearScheduleCache } from "./calendarScheduleCache";
import { removeSyncedCalendarAccount } from "./replicatedSync";
import { mailClient } from "./data/client";
import type { CalendarAccount, CalendarOption } from "./domain";
import { errorMessage, logBackgroundFailure } from "./errors";

/**
 * Owns connected calendar accounts and the per-account calendar selection.
 *
 * `onLastAccountRemoved` lets the caller close calendar-dependent surfaces
 * once no calendar account remains; it should be a stable callback.
 */
export function useCalendarAccounts({ onLastAccountRemoved }: { onLastAccountRemoved?(): void } = {}) {
  const [accounts, setAccounts] = useState<CalendarAccount[]>([]);
  const [calendars, setCalendars] = useState<CalendarOption[]>([]);
  const [calendarsError, setCalendarsError] = useState<string | null>(null);
  const [calendarsLoaded, setCalendarsLoaded] = useState(false);

  const accountScope = useRef<string | null>(null);
  const selectionScope = useRef<string | null>(null);

  const refreshAccounts = useCallback(async () => {
    const next = await mailClient.listCalendarAccounts();
    const scope = JSON.stringify(next);
    if (accountScope.current !== scope) clearScheduleCache();
    accountScope.current = scope;
    setAccounts(next);
    return next;
  }, []);

  useEffect(() => {
    void refreshAccounts().catch(logBackgroundFailure("Calendar account listing"));
  }, [refreshAccounts]);

  const refreshCalendars = useCallback(async () => {
    try {
      const next = await mailClient.listCalendarOptions();
      const scope = JSON.stringify(next);
      if (selectionScope.current !== scope) {
        clearScheduleCache();
        window.dispatchEvent(new Event(CALENDAR_AVAILABILITY_CHANGED));
      }
      selectionScope.current = scope;
      setCalendars(next);
      setCalendarsError(null);
      setCalendarsLoaded(true);
      return next;
    } catch (reason) {
      setCalendarsError(errorMessage(reason));
      throw reason;
    }
  }, []);

  // Surfaces outside the Calendar view, such as the inbox-zero agenda, read
  // the selection too, so load it as soon as a calendar is connected rather
  // than waiting for the Calendar view or Settings to ask.
  const hasConnectedAccount = accounts.some((account) => account.status === "connected");
  useEffect(() => {
    if (hasConnectedAccount && !calendarsLoaded) void refreshCalendars().catch(logBackgroundFailure("Calendar listing"));
  }, [hasConnectedAccount, calendarsLoaded, refreshCalendars]);

  const add = useCallback(async () => {
    await mailClient.addCalendarAccount();
    clearScheduleCache();
    await refreshAccounts();
    await refreshCalendars();
  }, [refreshAccounts, refreshCalendars]);

  const reconnect = useCallback(async (email: string) => {
    await mailClient.reconnectCalendarAccount(email);
    clearScheduleCache();
    await refreshAccounts();
    await refreshCalendars();
  }, [refreshAccounts, refreshCalendars]);

  const remove = useCallback(async (email: string) => {
    await mailClient.removeCalendarAccount(email);
    clearScheduleCache();
    const remaining = await refreshAccounts();
    setCalendars((current) => current.filter((calendar) => calendar.accountId !== email));
    if (remaining.length === 0) onLastAccountRemoved?.();
  }, [onLastAccountRemoved, refreshAccounts]);

  const removeEverywhere = useCallback(async (email: string) => {
    await removeSyncedCalendarAccount(email);
    clearScheduleCache();
    await refreshAccounts();
  }, [refreshAccounts]);

  const reorder = useCallback(async (emails: string[]) => {
    const accountsByEmail = new Map(accounts.map((account) => [account.email, account]));
    const reordered = emails
      .map((email) => accountsByEmail.get(email))
      .filter((account): account is CalendarAccount => account !== undefined);
    if (reordered.length !== accounts.length) return;

    const previous = accounts;
    setAccounts(reordered);
    try {
      await mailClient.reorderCalendarAccounts(emails);
      await refreshAccounts();
    } catch (error) {
      setAccounts(previous);
      throw error;
    }
  }, [accounts, refreshAccounts]);

  const setSelection = useCallback(async (accountId: string, calendarIds: string[]) => {
    const updated = await mailClient.setCalendarSelection(accountId, calendarIds);
    clearScheduleCache();
    setCalendars((current) => [
      ...current.filter((calendar) => calendar.accountId !== accountId),
      ...updated,
    ]);
  }, []);

  const setAvailabilitySelection = useCallback(async (accountId: string, calendarIds: string[]) => {
    const updated = await mailClient.setAvailabilityCalendarSelection(accountId, calendarIds);
    window.dispatchEvent(new Event(CALENDAR_AVAILABILITY_CHANGED));
    setCalendars((current) => [...current.filter((calendar) => calendar.accountId !== accountId), ...updated]);
  }, []);

  return {
    accounts,
    calendars,
    calendarsError,
    calendarsLoaded,
    refreshAccounts,
    refreshCalendars,
    add,
    reconnect,
    remove,
    removeEverywhere,
    reorder,
    setSelection,
    setAvailabilitySelection,
  };
}
