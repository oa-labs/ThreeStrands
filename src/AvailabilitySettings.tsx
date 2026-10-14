import { useEffect, useMemo, useState } from "react";
import { Plus, X } from "lucide-react";
import type { AvailabilityPreferences, AvailabilityWindow } from "./domain";
import { ICON_SIZE } from "./iconSizes";
import { MAX_WORKING_WINDOWS_PER_DAY } from "./settings";

const LAST_MINUTE = 23 * 60 + 59;

function clockMinutes(value: string): number {
  const [hour, minute] = value.split(":").map(Number);
  return hour * 60 + minute;
}

function clockValue(minutes: number): string {
  return `${String(Math.floor(minutes / 60)).padStart(2, "0")}:${String(minutes % 60).padStart(2, "0")}`;
}

/** An hour-long slot starting an hour after the day's latest slot ends, or
 * null when that would run past midnight. */
export function nextWorkingWindow(weekday: number, dayWindows: AvailabilityWindow[]): AvailabilityWindow | null {
  if (dayWindows.length === 0) return { weekday, start: "09:00", end: "17:00" };
  const start = Math.max(...dayWindows.map((window) => clockMinutes(window.end))) + 60;
  const end = start + 60;
  return end > LAST_MINUTE ? null : { weekday, start: clockValue(start), end: clockValue(end) };
}

export function windowsOverlap(dayWindows: AvailabilityWindow[]): boolean {
  const sorted = [...dayWindows].sort((a, b) => a.start.localeCompare(b.start));
  return sorted.some((window, index) => index > 0 && window.start < sorted[index - 1].end);
}

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
  const updateWindow = (target: AvailabilityWindow, patch: Partial<Pick<AvailabilityWindow, "start" | "end">>) => {
    onChange({
      ...preferences,
      workingWindows: preferences.workingWindows.map((window) => window === target ? { ...window, ...patch } : window),
    });
  };
  const addWindow = (window: AvailabilityWindow) => onChange({ ...preferences, workingWindows: [...preferences.workingWindows, window] });
  const removeWindows = (shouldRemove: (window: AvailabilityWindow) => boolean) => onChange({
    ...preferences,
    workingWindows: preferences.workingWindows.filter((window) => !shouldRemove(window)),
  });
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
              const mondayWindows = preferences.workingWindows.filter((window) => window.weekday === 1);
              const monday = mondayWindows.length > 0 ? mondayWindows : [{ weekday: 1, start: "09:00", end: "17:00" }];
              const weekends = preferences.workingWindows.filter((window) => window.weekday === 0 || window.weekday === 6);
              onChange({
                ...preferences,
                workingWindows: [
                  ...weekends,
                  ...[1, 2, 3, 4, 5].flatMap((weekday) => monday.map(({ start, end }) => ({ weekday, start, end }))),
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
          const dayWindows = preferences.workingWindows.filter((candidate) => candidate.weekday === weekday);
          const next = dayWindows.length < MAX_WORKING_WINDOWS_PER_DAY ? nextWorkingWindow(weekday, dayWindows) : null;
          return (
            <div className="availability-window" key={label}>
              <label><input type="checkbox" checked={dayWindows.length > 0} onChange={(event) => {
                if (event.target.checked) addWindow({ weekday, start: "09:00", end: "17:00" });
                else removeWindows((candidate) => candidate.weekday === weekday);
              }} /> {label}</label>
              {dayWindows.length > 0 ? (
                <div className="availability-slots" role="group" aria-label={`${label} Hours`}>
                  {dayWindows.map((window, index) => {
                    const suffix = index === 0 ? "" : ` ${index + 1}`;
                    return (
                      <div className="availability-slot" key={index}>
                        <input type="time" aria-label={`${label} Start${suffix}`} value={window.start} onChange={(event) => updateWindow(window, { start: event.target.value })} />
                        <span>to</span>
                        <input type="time" aria-label={`${label} End${suffix}`} value={window.end} onChange={(event) => updateWindow(window, { end: event.target.value })} />
                        {dayWindows.length > 1 ? (
                          <button className="btn-icon btn-icon-sm" type="button" title="Remove slot" aria-label={`Remove ${label} slot ${index + 1}`} onClick={() => removeWindows((candidate) => candidate === window)}>
                            <X size={ICON_SIZE.sm} />
                          </button>
                        ) : <span />}
                        {index === dayWindows.length - 1 ? (
                          <button className="btn-icon btn-icon-sm" type="button" title="Add slot" aria-label={`Add ${label} slot`} disabled={!next} onClick={() => next && addWindow(next)}>
                            <Plus size={ICON_SIZE.sm} />
                          </button>
                        ) : null}
                      </div>
                    );
                  })}
                  {windowsOverlap(dayWindows) ? <span className="settings-hint">Overlapping slots count as one.</span> : null}
                </div>
              ) : <span className="settings-hint">Unavailable</span>}
            </div>
          );
        })}
      </div>
      <h3>Meeting Defaults</h3>
      <label className="settings-field settings-field-inline settings-field-fixed"><span>Default Duration</span><select value={preferences.defaultDurationMinutes} onChange={(event) => onChange({ ...preferences, defaultDurationMinutes: Number(event.target.value) })}>{[15, 30, 45, 60, 90, 120].map((value) => <option key={value} value={value}>{value} Minutes</option>)}</select></label>
      <label className="settings-field settings-field-inline settings-field-fixed"><span>Slot Increment</span><select value={preferences.slotIncrementMinutes} onChange={(event) => onChange({ ...preferences, slotIncrementMinutes: Number(event.target.value) })}>{[5, 10, 15, 30, 60].map((value) => <option key={value} value={value}>{value} Minutes</option>)}</select></label>
    </section>
  );
}
