import { useCallback, useEffect, useState } from "react";
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

  const refreshAccounts = useCallback(async () => {
    const next = await mailClient.listCalendarAccounts();
    setAccounts(next);
    return next;
  }, []);

  useEffect(() => {
    void refreshAccounts().catch(logBackgroundFailure("Calendar account listing"));
  }, [refreshAccounts]);

  const refreshCalendars = useCallback(async () => {
    try {
      const next = await mailClient.listCalendarOptions();
      setCalendars(next);
      setCalendarsError(null);
      return next;
    } catch (reason) {
      setCalendarsError(errorMessage(reason));
      throw reason;
    }
  }, []);

  const add = useCallback(async () => {
    await mailClient.addCalendarAccount();
    await refreshAccounts();
    await refreshCalendars();
  }, [refreshAccounts, refreshCalendars]);

  const reconnect = useCallback(async (email: string) => {
    await mailClient.reconnectCalendarAccount(email);
    await refreshAccounts();
    await refreshCalendars();
  }, [refreshAccounts, refreshCalendars]);

  const remove = useCallback(async (email: string) => {
    await mailClient.removeCalendarAccount(email);
    const remaining = await refreshAccounts();
    setCalendars((current) => current.filter((calendar) => calendar.accountId !== email));
    if (remaining.length === 0) onLastAccountRemoved?.();
  }, [onLastAccountRemoved, refreshAccounts]);

  const removeEverywhere = useCallback(async (email: string) => {
    await removeSyncedCalendarAccount(email);
    await refreshAccounts();
  }, [refreshAccounts]);

  const setSelection = useCallback(async (accountId: string, calendarIds: string[]) => {
    const updated = await mailClient.setCalendarSelection(accountId, calendarIds);
    setCalendars((current) => [
      ...current.filter((calendar) => calendar.accountId !== accountId),
      ...updated,
    ]);
  }, []);

  return {
    accounts,
    calendars,
    calendarsError,
    refreshAccounts,
    refreshCalendars,
    add,
    reconnect,
    remove,
    removeEverywhere,
    setSelection,
  };
}
