import type { CalendarAccount, CalendarOption } from "./domain";

/**
 * Which connected accounts the calendar shows: an account is hidden when it lists calendars but none are selected.
 * Accounts whose calendars have not loaded count as neither.
 */
export function calendarAccountVisibility(accounts: CalendarAccount[], calendars: CalendarOption[]) {
  const listed = accounts.filter((account) => calendars.some((calendar) => calendar.accountId === account.email));
  const hidden = listed.filter((account) => !calendars.some((calendar) => calendar.accountId === account.email && calendar.selected));
  const selectedAccountEmails = [...new Set(calendars.filter((calendar) => calendar.selected).map((calendar) => calendar.accountId))];
  // A partial scope is a count rather than addresses, so it does not read as the account you are "in".
  const label = selectedAccountEmails.length === 0
    ? "None selected"
    : hidden.length === 0 ? "All calendar accounts" : `${listed.length - hidden.length} of ${listed.length} accounts`;
  return { total: listed.length, shown: listed.length - hidden.length, hidden, selectedAccountEmails, label };
}

/** The calendar Show All turns on for a hidden account: its primary, or its first when none is marked primary. */
export function defaultCalendarFor(accountId: string, calendars: CalendarOption[]): CalendarOption | undefined {
  const accountCalendars = calendars.filter((calendar) => calendar.accountId === accountId);
  return accountCalendars.find((calendar) => calendar.primary) ?? accountCalendars[0];
}

/**
 * Turns on only each hidden account's default calendar, so secondary and subscribed calendars stay as the person
 * left them; some accounts list dozens.
 */
export function showHiddenCalendarAccounts(
  accounts: CalendarAccount[],
  calendars: CalendarOption[],
  onToggle: (accountId: string, calendarId: string, selected: boolean) => void,
): void {
  for (const account of calendarAccountVisibility(accounts, calendars).hidden) {
    const calendar = defaultCalendarFor(account.email, calendars);
    if (calendar) onToggle(account.email, calendar.id, true);
  }
}
