import { Check } from "lucide-react";
import { ContextRow } from "./ContextSections";
import { ICON_SIZE } from "./iconSizes";
import { useEffect, useRef, useState } from "react";
import type { AvailabilityCandidate, AvailabilityPreferences, CheckedProposedTime, ProposedTime } from "./domain";
import { mailClient } from "./data/client";
import { CALENDAR_AVAILABILITY_CHANGED } from "./calendarAvailability";
import { listSupportedTimeZones } from "./calendarTime";
import { AvailabilityProfileSelect } from "./AvailabilityProfileChoice";
import { errorMessage } from "./errors";

export function ProposedTimesCard({ times, preferences, intoDraft, onReplyWithTimes, onFindOtherTimes, calendarConnected, onOpenCalendarSettings }: {
  times: ProposedTime[];
  preferences: AvailabilityPreferences;
  intoDraft?: boolean;
  calendarConnected: boolean;
  onReplyWithTimes(times: AvailabilityCandidate[]): void;
  onFindOtherTimes(): void;
  onOpenCalendarSettings(): void;
}) {
  const generation = useRef(0);
  const [zoneOverride, setZone] = useState<string | null>(null);
  const zone = zoneOverride ?? preferences.timeZone;
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
  const durations = [...new Set(times.map((time) => time.durationMinutes))];
  const format = (row: CheckedProposedTime) => {
    if (!row.start || !row.end) return null;
    const day = new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric", timeZone: preferences.timeZone }).format(new Date(row.start));
    const clock = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit", timeZone: preferences.timeZone });
    return `${day} · ${clock.formatRange(new Date(row.start), new Date(row.end))}`;
  };
  return <section aria-label="Proposed times" className="proposed-times">
    <p className="context-section-note proposed-times-summary">{times.length} proposed {times.length === 1 ? "time" : "times"}{durations.length === 1 && durations[0] ? ` · ${durations[0]} minutes each` : ""}</p>
    {times.some((time) => !time.timeZone) ? <label className="proposed-times-zone">Assuming timezone
      <select value={zone} disabled={checking} onChange={(event) => setZone(event.target.value)}>
        {[...new Set([zone, ...listSupportedTimeZones()])].map((value) => <option key={value}>{value}</option>)}
      </select>
    </label> : null}
    <AvailabilityProfileSelect disabled={checking} />
    <p className="context-status">Shown in {preferences.timeZone}. Select the times you want to offer.</p>
    {!calendarConnected ? <button type="button" className="btn btn-sm" onClick={onOpenCalendarSettings}>Connect Calendar</button> : !rows && !error ? <p className="context-status" role="status">Checking proposed times…</p> : null}
    <ol className="proposed-times-list">
      {times.map((time, index) => {
        const row = rows?.[index];
        const status = row?.issue ?? (row?.check?.status === "free" ? "Free" : row?.check?.status === "conflicting" ? "Conflict" : row?.check?.status === "partiallyChecked" ? "Partially checked — no conflict found on checked calendars" : row?.check?.status === "unverified" ? "Unable to check — no calendar could be checked" : "Not checked");
        // Keep one sender option number. Dates such as "10.31" are not list markers.
        const marker = /^\s*(?:\((\d+)\)|(\d+)[.)])\s+/.exec(time.sourceText);
        const number = marker?.[1] ?? marker?.[2] ?? String(index + 1);
        const title = `${number}. ${row && format(row) || time.sourceText.slice(marker?.[0].length ?? 0)}`;
        return <li key={index}>
          <ContextRow
            className="proposed-time-row"
            icon={selected.has(index) ? <Check size={ICON_SIZE.sm} /> : null}
            title={title}
            detail={`${status}${row?.outsideWorkingHours ? " · Outside your usual hours" : ""}`}
            wrapTitle
            pressed={selected.has(index)}
            disabled={checking || !row || !selectable(row)}
            onActivate={() => setSelected((previous) => {
              const next = new Set(previous); if (next.has(index)) next.delete(index); else next.add(index); return next;
            })}
          />
          {row?.start ? <details><summary>From the email</summary><p>{time.sourceText}</p></details> : null}
        </li>;
      })}
    </ol>
    {rows && [...new Set(rows.flatMap((row) => row.check?.uncheckedCalendars ?? []))].length ? <p role="status">Couldn’t check: {[...new Set(rows.flatMap((row) => row.check?.uncheckedCalendars ?? []))].join(", ")}. These times may conflict with those calendars.</p> : null}
    {rows && rows.some((row) => row.check?.errors.length) ? <details><summary>Calendar check details</summary>{[...new Set(rows.flatMap((row) => row.check?.errors ?? []))].map((message) => <p key={message}>{message}</p>)}</details> : null}
    {error ? <p role="alert">{error}</p> : null}
    <div className="meeting-scheduler-actions">
      <button type="button" className="btn btn-sm" disabled={!selected.size || checking} onClick={() => void reply()}>{checking ? "Checking…" : intoDraft ? "Insert Selected Times" : "Draft Reply With Selected Times"}</button>
      <button type="button" className="btn btn-sm" disabled={checking || !calendarConnected} onClick={() => setAttempt((value) => value + 1)}>Check Again</button>
      <button type="button" className="btn btn-sm" disabled={checking} onClick={onFindOtherTimes}>Find Other Times</button>
    </div>
  </section>;
}
