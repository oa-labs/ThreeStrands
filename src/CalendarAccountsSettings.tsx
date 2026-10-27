import { isHolidayCalendar } from "./calendarAvailability";
import { CircleAlert, CalendarDays, ChevronDown, ChevronUp, Plus } from "lucide-react";
import { CalendarColorRow } from "./CalendarColorMenu";
import { useState } from "react";
import type { AuthStatus, CalendarAccount, CalendarOption } from "./domain";
import { moveItem, useSettingsOperation } from "./settingsOperations";
import { ICON_SIZE } from "./iconSizes";
import { AccountStatusBadge, AccountDisconnectConfirm } from "./accountSettingsParts";

export function CalendarAccountsSettings({
  authStatus,
  accounts,
  calendars,
  calendarsError,
  calendarsLoaded,
  onAdd,
  onReconnect,
  onRemove,
  onRemoveEverywhere,
  onReorder,
  onSetSelection,
  onSetAvailabilitySelection,
}: {
  authStatus: AuthStatus | null;
  accounts: CalendarAccount[];
  calendars: CalendarOption[];
  calendarsError: string | null;
  calendarsLoaded: boolean;
  onAdd(): Promise<void>;
  onReconnect(email: string): Promise<void>;
  onRemove(email: string): Promise<void>;
  onRemoveEverywhere(email: string): Promise<void>;
  onReorder(emails: string[]): Promise<void>;
  onSetSelection(accountId: string, calendarIds: string[]): Promise<void>;
  onSetAvailabilitySelection(accountId: string, calendarIds: string[]): Promise<void>;
}) {
  const { pending: busyEmail, error, runFor } = useSettingsOperation();
  const [confirmEmail, setConfirmEmail] = useState<string | null>(null);

  const move = (index: number, direction: -1 | 1) => {
    const account = accounts[index];
    const next = moveItem(accounts, index, direction);
    if (account && next) runFor(account.email, () => onReorder(next.map((item) => item.email)));
  };

  return (
    <section className="settings-section accounts-manager" aria-label="Calendar Accounts">
      <div className="accounts-manager-header">
        <div>
          <h3>Google Calendar</h3>
          <p>
            Calendar access is connected separately from mail and can create events.
            Each account gets its own Calendar consent and keychain credential.
          </p>
        </div>
        <button
          type="button"
          className="btn btn-primary settings-add-account"
          disabled={busyEmail !== null}
          onClick={() => runFor("__add__", onAdd)}
        >
          <Plus size={ICON_SIZE.md} />
          {busyEmail === "__add__" ? "Waiting for Google…" : "Connect Calendar"}
        </button>
      </div>
      {accounts.length === 0 && authStatus && !authStatus.configured ? (
        <div className="notice accounts-config-notice">
          <CircleAlert size={ICON_SIZE.md} />
          <div>
            <strong>Google OAuth is not configured</strong>
            <p>Configure the Google Desktop app credentials used for mail, then restart ThreeStrands.</p>
          </div>
        </div>
      ) : null}
      {accounts.length === 0 ? (
        <div className="accounts-empty">
          <span className="accounts-empty-icon"><CalendarDays size={ICON_SIZE.lg} /></span>
          <strong>No calendars connected</strong>
          <p>Connect Google Calendar to use the T shortcut and see your live schedule.</p>
        </div>
      ) : (
        <ul className="accounts-list">
          {accounts.map((account, index) => {
            const accountCalendars = calendars.filter((calendar) => calendar.accountId === account.email);
            return (
            <li className="account-card" key={account.email}>
              <div className="account-card-row">
                <span className="account-card-avatar calendar-account-avatar" aria-hidden="true">
                  <CalendarDays size={ICON_SIZE.lg} />
                </span>
                <div className="account-card-identity">
                  <div className="account-card-heading">
                    <strong title={account.email}>{account.email}</strong>
                    <AccountStatusBadge status={account.status} />
                  </div>
                  <span className="account-card-email">Calendar events and availability</span>
                </div>
                {account.status === "needs_reauth" ? (
                  <button
                    type="button"
                    className="btn btn-sm btn-primary account-reconnect"
                    disabled={busyEmail !== null}
                    onClick={() => runFor(`reconnect:${account.email}`, () => onReconnect(account.email))}
                  >
                    {busyEmail === `reconnect:${account.email}` ? "Waiting for Google…" : "Reconnect"}
                  </button>
                ) : null}
                <span className="accounts-list-actions">
                  {accounts.length > 1 ? (
                    <span className="account-reorder" role="group" aria-label={`Order for ${account.email}`}>
                      <button className="btn-icon"
                        type="button"
                        title="Move up"
                        aria-label={`Move ${account.email} up`}
                        disabled={index === 0 || busyEmail !== null}
                        onClick={() => move(index, -1)}
                      >
                        <ChevronUp size={ICON_SIZE.sm} />
                      </button>
                      <button className="btn-icon"
                        type="button"
                        title="Move down"
                        aria-label={`Move ${account.email} down`}
                        disabled={index === accounts.length - 1 || busyEmail !== null}
                        onClick={() => move(index, 1)}
                      >
                        <ChevronDown size={ICON_SIZE.sm} />
                      </button>
                    </span>
                  ) : null}
                  <button
                    type="button"
                    className="btn btn-sm btn-danger"
                    disabled={busyEmail !== null}
                    aria-expanded={confirmEmail === account.email}
                    onClick={() => setConfirmEmail(account.email)}
                  >
                    Disconnect…
                  </button>
                </span>
              </div>
              {confirmEmail === account.email ? (
                <AccountDisconnectConfirm
                  kind="calendar"
                  email={account.email}
                  disabled={busyEmail !== null}
                  onCancel={() => setConfirmEmail(null)}
                  onDisconnect={() => {
                    runFor(account.email, () => onRemove(account.email));
                    setConfirmEmail(null);
                  }}
                  onRemoveEverywhere={() => {
                    runFor(account.email, () => onRemoveEverywhere(account.email));
                    setConfirmEmail(null);
                  }}
                />
              ) : null}
              {account.status === "connected" ? (
                <fieldset className="calendar-picker">
                  <legend>Calendars shown in the sidebar</legend>
                  {accountCalendars.length === 0 ? (
                    <p>
                      {calendarsError
                        ? "Calendars couldn’t be loaded."
                        : calendarsLoaded
                          ? "No calendars found for this account."
                          : "Loading calendars…"}
                    </p>
                  ) : accountCalendars.map((calendar) => (
                      <CalendarColorRow key={calendar.id} accountId={account.email} calendarId={calendar.id} calendarName={calendar.name}>
                        <label>
                          <input
                            type="checkbox"
                            checked={calendar.selected}
                            disabled={busyEmail !== null}
                            onChange={(event) => {
                              const selected = calendars
                                .filter((candidate) =>
                                  candidate.accountId === account.email
                                  && candidate.selected
                                  && candidate.id !== calendar.id
                                )
                                .map((candidate) => candidate.id);
                              if (event.target.checked) selected.push(calendar.id);
                              runFor(account.email, () => onSetSelection(account.email, selected));
                            }}
                          />
                          <span>{calendar.name}{calendar.primary ? " (Primary)" : ""}</span>
                        </label>
                      </CalendarColorRow>
                    ))}
                </fieldset>
              ) : null}
              {account.status === "connected" && accountCalendars.length > 0 ? <fieldset className="calendar-picker">
                <legend>Calendars used for availability</legend>
                <p>Primary calendars are used by default. Holiday calendars are always included when available. These choices do not change which calendars are shown. Coworkers are checked using their primary calendar only.</p>
                {accountCalendars.map((calendar) => <label key={calendar.id}>
                  <input type="checkbox" checked={isHolidayCalendar(calendar.id) || (calendar.availabilitySelected ?? calendar.primary)} disabled={busyEmail !== null || isHolidayCalendar(calendar.id)} onChange={(event) => {
                    const ids = accountCalendars.filter((candidate) => candidate.id !== calendar.id && (candidate.availabilitySelected ?? candidate.primary)).map((candidate) => candidate.id);
                    if (event.target.checked) ids.push(calendar.id);
                    runFor(account.email, () => onSetAvailabilitySelection(account.email, ids));
                  }} />
                  <span>{calendar.name}{calendar.primary ? " (Primary)" : isHolidayCalendar(calendar.id) ? " (Included when available)" : ""}</span>
                </label>)}
              </fieldset> : null}
            </li>
            );
          })}
        </ul>
      )}
      {calendarsError ? <p className="form-error" role="alert">{calendarsError}</p> : null}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
    </section>
  );
}
