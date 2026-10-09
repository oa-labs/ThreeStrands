import { useEffect, useMemo, useState } from "react";
import type { AvailabilityPreferences } from "./domain";

export function AvailabilitySettings({
  preferences,
  onChange,
}: {
  preferences: AvailabilityPreferences;
  onChange(value: AvailabilityPreferences): void;
}) {
  const weekdayLabels = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
  const [timeZoneDraft, setTimeZoneDraft] = useState(preferences.timeZone);
  const [timeZoneError, setTimeZoneError] = useState<string | null>(null);
  const systemTimeZone = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  const timeZones = useMemo(() => {
    try {
      return Intl.supportedValuesOf("timeZone");
    } catch {
      return ["UTC"];
    }
  }, []);

  useEffect(() => setTimeZoneDraft(preferences.timeZone), [preferences.timeZone]);

  const commitTimeZone = (value: string) => {
    const normalized = value.trim();
    try {
      new Intl.DateTimeFormat("en-US", { timeZone: normalized }).format();
      setTimeZoneDraft(normalized);
      setTimeZoneError(null);
      if (normalized !== preferences.timeZone) onChange({ ...preferences, timeZone: normalized });
    } catch {
      setTimeZoneError("Choose a valid timezone, such as America/New_York.");
    }
  };
  const updateWindow = (weekday: number, patch: Partial<{ start: string; end: string }>) => {
    const current = preferences.workingWindows.find((window) => window.weekday === weekday);
    const next = current
      ? preferences.workingWindows.map((window) => window.weekday === weekday ? { ...window, ...patch } : window)
      : [...preferences.workingWindows, { weekday, start: patch.start ?? "09:00", end: patch.end ?? "17:00" }];
    onChange({ ...preferences, workingWindows: next });
  };
  return (
    <section className="settings-section" aria-label="Availability">
      <h3>Timezone</h3>
      <label className="settings-field">
        <span>Timezone</span>
        <input
          list="availability-timezones"
          value={timeZoneDraft}
          aria-label="Availability Timezone"
          aria-invalid={timeZoneError ? "true" : undefined}
          onChange={(event) => {
            setTimeZoneDraft(event.target.value);
            setTimeZoneError(null);
          }}
          onBlur={(event) => commitTimeZone(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              commitTimeZone(event.currentTarget.value);
            }
          }}
        />
        <datalist id="availability-timezones">
          {timeZones.map((timeZone) => <option key={timeZone} value={timeZone} />)}
        </datalist>
      </label>
      <div className="settings-row">
        <button className="btn" type="button" onClick={() => commitTimeZone(systemTimeZone)}>Use system timezone</button>
        <span className="settings-hint">Times include daylight-saving transitions.</span>
      </div>
      {timeZoneError ? <p className="form-error" role="alert">{timeZoneError}</p> : null}
      <div className="settings-section-heading-row">
        <h3>Working Hours</h3>
        <span className="settings-section-heading-actions">
          <button className="btn btn-sm"
            type="button"
            onClick={() => {
              const monday = preferences.workingWindows.find((window) => window.weekday === 1)
                ?? { weekday: 1, start: "09:00", end: "17:00" };
              const weekends = preferences.workingWindows.filter((window) => window.weekday === 0 || window.weekday === 6);
              onChange({
                ...preferences,
                workingWindows: [
                  ...weekends,
                  ...[1, 2, 3, 4, 5].map((weekday) => ({ weekday, start: monday.start, end: monday.end })),
                ].sort((a, b) => a.weekday - b.weekday),
              });
            }}
          >
            Copy Monday to weekdays
          </button>
          <button className="btn btn-sm" type="button" onClick={() => onChange({ ...preferences, workingWindows: [] })}>Clear</button>
        </span>
      </div>
      <div className="availability-windows">
        {weekdayLabels.map((label, weekday) => {
          const window = preferences.workingWindows.find((candidate) => candidate.weekday === weekday);
          return (
            <div className="availability-window" key={label}>
              <label><input type="checkbox" checked={Boolean(window)} onChange={(event) => {
                if (event.target.checked) updateWindow(weekday, {});
                else onChange({ ...preferences, workingWindows: preferences.workingWindows.filter((candidate) => candidate.weekday !== weekday) });
              }} /> {label}</label>
              {window ? <>
                <input type="time" aria-label={`${label} start`} value={window.start} onChange={(event) => updateWindow(weekday, { start: event.target.value })} />
                <span>to</span>
                <input type="time" aria-label={`${label} end`} value={window.end} onChange={(event) => updateWindow(weekday, { end: event.target.value })} />
              </> : <span className="settings-hint">Unavailable</span>}
            </div>
          );
        })}
      </div>
      <h3>Meeting Defaults</h3>
      <label className="settings-field settings-field-inline settings-field-fixed"><span>Default Duration</span><select value={preferences.defaultDurationMinutes} onChange={(event) => onChange({ ...preferences, defaultDurationMinutes: Number(event.target.value) })}>{[15, 30, 45, 60, 90, 120].map((value) => <option key={value} value={value}>{value} minutes</option>)}</select></label>
      <label className="settings-field settings-field-inline settings-field-fixed"><span>Slot Increment</span><select value={preferences.slotIncrementMinutes} onChange={(event) => onChange({ ...preferences, slotIncrementMinutes: Number(event.target.value) })}>{[5, 10, 15, 30, 60].map((value) => <option key={value} value={value}>{value} minutes</option>)}</select></label>
    </section>
  );
}
