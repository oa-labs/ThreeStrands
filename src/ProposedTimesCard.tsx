import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { AvailabilityCandidate, AvailabilityPreferences, CheckedProposedTime, ProposedTime } from "./domain";
import { mailClient } from "./data/client";
import { CALENDAR_AVAILABILITY_CHANGED } from "./calendarAvailability";
import { friendlyZoneName, timeZoneChoices } from "./calendarTime";
import { formatLength, MEETING_LENGTHS } from "./MeetingScheduler";
import { AvailabilityProfileSelect } from "./AvailabilityProfileChoice";
import { errorMessage } from "./errors";

export function ProposedTimesCard({ times: offered, preferences, intoDraft, onReplyWithTimes, onFindOtherTimes, calendarConnected, onOpenCalendarSettings }: {
  times: ProposedTime[];
  preferences: AvailabilityPreferences;
  intoDraft?: boolean;
  calendarConnected: boolean;
  onReplyWithTimes(times: AvailabilityCandidate[]): void;
  onFindOtherTimes(): void;
  onOpenCalendarSettings(): void;
}) {
  const generation = useRef(0);
  const ids = useId();
  const [zoneOverride, setZone] = useState<string | null>(null);
  const zone = zoneOverride ?? preferences.timeZone;
  const zones = useMemo(() => timeZoneChoices(zone), [zone]);
  // Most emails never say how long; assume the usual length visibly instead of refusing the time.
  const [lengthOverride, setLength] = useState<number | null>(null);
  const length = lengthOverride ?? preferences.defaultDurationMinutes;
  const lengthUnstated = offered.some((time) => !time.durationMinutes);
  const times = useMemo(
    () => lengthUnstated ? offered.map((time) => time.durationMinutes ? time : { ...time, durationMinutes: length }) : offered,
    [offered, lengthUnstated, length],
  );
  const [rows, setRows] = useState<CheckedProposedTime[] | null>(null);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [attempt, setAttempt] = useState(0);
  const [error, setError] = useState("");
  const [checking, setChecking] = useState(false);
  useEffect(() => {
    const refresh = () => { setRows(null); setSelected(new Set()); setAttempt((value) => value + 1); };
    window.addEventListener(CALENDAR_AVAILABILITY_CHANGED, refresh);
    return () => window.removeEventListener(CALENDAR_AVAILABILITY_CHANGED, refresh);
  }, []);
  useEffect(() => {
    const version = ++generation.current;
    const current = () => generation.current === version;
    setRows(null);
    setSelected(new Set());
    setError("");
    if (calendarConnected) {
      void mailClient.checkProposedTimes({ times, assumedTimeZone: zone, preferences }).then((result) => {
        if (current()) setRows(result);
      }).catch((reason) => { if (current()) setError(errorMessage(reason)); });
    }
    return () => { generation.current = version + 1; };
  }, [times, zone, preferences, attempt, calendarConnected]);

  const selectable = (row: CheckedProposedTime) => row.start && row.end && !row.issue && row.check?.status === "free" && Date.parse(row.start) > Date.now();
  const reply = async () => {
    const version = generation.current;
    setChecking(true);
    setError("");
    try {
      // Recheck immediately before claiming any time works; keep other rows visible.
      const fresh = await mailClient.checkProposedTimes({ times, assumedTimeZone: zone, preferences });
      if (version !== generation.current) return;
      setRows(fresh);
      if ([...selected].some((index) => !fresh[index] || !selectable(fresh[index]))) {
        setSelected(new Set([...selected].filter((index) => fresh[index] && selectable(fresh[index]))));
        setError("Availability changed. Review the updated times before drafting your reply.");
        return;
      }
      onReplyWithTimes([...selected].sort((a, b) => a - b).map((index) => ({ start: fresh[index].start!, end: fresh[index].end!, status: "verified" })));
    } catch (reason) { if (version === generation.current) setError(errorMessage(reason)); }
    finally { setChecking(false); }
  };
  const durations = [...new Set(offered.map((time) => time.durationMinutes))];
  const lengths = [...new Set([...MEETING_LENGTHS, length])].sort((a, b) => a - b);
  const anySelectable = Boolean(rows?.some(selectable));
  // Mention the reader's own zone only when it differs from the zone the email's times are in.
  const converted = offered.some((time) => (time.timeZone ?? zone) !== preferences.timeZone);
  const format = (row: CheckedProposedTime) => {
    if (!row.start || !row.end) return null;
    const day = new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric", timeZone: preferences.timeZone }).format(new Date(row.start));
    const clock = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit", timeZone: preferences.timeZone });
    return `${day} · ${clock.formatRange(new Date(row.start), new Date(row.end))}`;
  };
  return <section aria-label="Proposed times" className="proposed-times">
    <p className="context-section-note proposed-times-summary">{offered.length} proposed {offered.length === 1 ? "time" : "times"}{durations.length === 1 && durations[0] ? ` · ${formatLength(durations[0])} each` : ""}</p>
    {offered.some((time) => !time.timeZone) ? <label className="proposed-times-zone">Sender’s timezone
      <select value={zone} disabled={checking} title="The email didn’t say which timezone its times are in." onChange={(event) => setZone(event.target.value)}>
        <optgroup label="Common">
          {zones.common.map((choice) => <option key={choice.value} value={choice.value}>{choice.label}</option>)}
        </optgroup>
        <optgroup label="All timezones">
          {zones.all.map((choice) => <option key={choice.value} value={choice.value}>{choice.label}</option>)}
        </optgroup>
      </select>
    </label> : null}
    {lengthUnstated ? <label className="proposed-times-zone">Meeting length
      <select value={length} disabled={checking} title="The email didn’t say how long the meeting is." onChange={(event) => setLength(Number(event.target.value))}>
        {lengths.map((minutes) => <option key={minutes} value={minutes}>{formatLength(minutes)}</option>)}
      </select>
    </label> : null}
    <AvailabilityProfileSelect disabled={checking} />
    {converted ? <p className="context-status">Shown in your timezone, {friendlyZoneName(preferences.timeZone)}.</p> : null}
    {!calendarConnected ? <button type="button" className="btn btn-sm" onClick={onOpenCalendarSettings}>Connect Calendar</button> : !rows && !error ? <p className="context-status" role="status">Checking proposed times…</p> : anySelectable ? <p className="context-status">Check the times you want to offer.</p> : null}
    <ol className="proposed-times-list">
      {offered.map((time, index) => {
        const row = rows?.[index];
        const status = row?.issue ?? (row?.check?.status === "free" ? "Free" : row?.check?.status === "conflicting" ? "Conflict" : row?.check?.status === "partiallyChecked" ? "Partially checked — no conflict found on checked calendars" : row?.check?.status === "unverified" ? "Unable to check — no calendar could be checked" : "Not checked");
        // Keep one sender option number. Dates such as "10.31" are not list markers.
        const marker = /^\s*(?:\((\d+)\)|(\d+)[.)])\s+/.exec(time.sourceText);
        const number = marker?.[1] ?? marker?.[2] ?? String(index + 1);
        const title = `${number}. ${row && format(row) || time.sourceText.slice(marker?.[0].length ?? 0)}`;
        const checked = selected.has(index);
        const detailId = `${ids}-${index}`;
        const detail = `${status}${row?.outsideWorkingHours ? " · Outside your usual hours" : ""}${row?.start && !time.durationMinutes ? ` · Assumes ${formatLength(length)}` : ""}`;
        return <li key={index} className={`proposed-time${checked ? " proposed-time-checked" : ""}`}>
          <label className="proposed-time-option">
            <input type="checkbox" checked={checked} disabled={checking || !row || !selectable(row)} aria-describedby={detailId}
              onChange={() => setSelected((previous) => {
                const next = new Set(previous); if (next.has(index)) next.delete(index); else next.add(index); return next;
              })} />
            <span className="proposed-time-title">{title}</span>
          </label>
          <span className="proposed-time-detail" id={detailId}>{detail}</span>
          {row?.start ? <details><summary>From the email</summary><p>{time.sourceText}</p></details> : null}
        </li>;
      })}
    </ol>
    {rows && [...new Set(rows.flatMap((row) => row.check?.uncheckedCalendars ?? []))].length ? <p role="status">Couldn’t check: {[...new Set(rows.flatMap((row) => row.check?.uncheckedCalendars ?? []))].join(", ")}. These times may conflict with those calendars.</p> : null}
    {rows && rows.some((row) => row.check?.errors.length) ? <details><summary>Calendar check details</summary>{[...new Set(rows.flatMap((row) => row.check?.errors ?? []))].map((message) => <p key={message}>{message}</p>)}</details> : null}
    {error ? <p role="alert">{error}</p> : null}
    <div className="meeting-scheduler-actions proposed-times-actions">
      <button type="button" className="btn btn-sm btn-primary" disabled={!selected.size || checking} onClick={() => void reply()}>{checking ? "Checking…" : intoDraft ? "Insert Selected Times" : "Draft Reply With Selected Times"}</button>
      <button type="button" className="btn btn-sm" disabled={checking || !calendarConnected} onClick={() => setAttempt((value) => value + 1)}>Check Again</button>
      <button type="button" className="btn-link context-link-button" disabled={checking} onClick={onFindOtherTimes}>Find Other Times</button>
    </div>
  </section>;
}
