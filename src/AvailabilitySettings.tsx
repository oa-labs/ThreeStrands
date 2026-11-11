import { useEffect, useId, useMemo, useState } from "react";
import { Plus, X } from "lucide-react";
import { MEETING_TYPES, type Account, type MeetingType, type AvailabilityPreferences, type AvailabilityProfile, type AvailabilityProfiles, type AvailabilityWindow } from "./domain";
import { formatClockTime } from "./agendaEvents";
import { ICON_SIZE } from "./iconSizes";
import { InlineConfirm } from "./InlineConfirm";
import {
  DEFAULT_AVAILABILITY_PROFILES,
  DRIVE_TIME_BUFFER_OPTIONS,
  DEFAULT_MEETING_DURATION_OPTIONS,
  MAX_AVAILABILITY_PROFILE_NAME_CHARS,
  MAX_AVAILABILITY_PROFILES,
  MAX_WORKING_WINDOWS_PER_DAY,
  availabilityProfileId,
  availabilityWithProfile,
} from "./settings";

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

type ProfileHours = Pick<AvailabilityProfile, "workingWindows" | "defaultDurationMinutes" | "driveTimeBufferMinutes">;

/** A new profile starting from the default profile's hours, named so it doesn't clash. */
export function addAvailabilityProfile(preferences: AvailabilityPreferences, profiles: AvailabilityProfiles, id: string): AvailabilityProfiles {
  const taken = new Set([profiles.defaultProfile, ...profiles.otherProfiles].map((profile) => profile.name.toLowerCase()));
  let name = "New Profile";
  for (let suffix = 2; taken.has(name.toLowerCase()); suffix += 1) name = `New Profile ${suffix}`;
  return {
    ...profiles,
    otherProfiles: [...profiles.otherProfiles, {
      id,
      name,
      workingWindows: preferences.workingWindows.map((window) => ({ ...window })),
      defaultDurationMinutes: preferences.defaultDurationMinutes,
      driveTimeBufferMinutes: preferences.driveTimeBufferMinutes,
    }],
  };
}

/** Removes a profile; accounts that used it go back to the default profile. */
export function deleteAvailabilityProfile(profiles: AvailabilityProfiles, id: string): AvailabilityProfiles {
  return {
    ...profiles,
    otherProfiles: profiles.otherProfiles.filter((profile) => profile.id !== id),
    ...(profiles.meetingTypeProfiles ? { meetingTypeProfiles: Object.fromEntries(Object.entries(profiles.meetingTypeProfiles).filter(([, profileId]) => profileId !== id)) } : {}),
    accountDefaults: Object.fromEntries(Object.entries(profiles.accountDefaults).filter(([, profileId]) => profileId !== id)),
  };
}

/**
 * Swaps the default profile with another. The default profile's hours live in
 * the shared preferences, so they trade places with the other profile's
 * hours; ids travel with their hours, so account defaults keep their profile.
 */
export function makeDefaultAvailabilityProfile(
  preferences: AvailabilityPreferences,
  profiles: AvailabilityProfiles,
  id: string,
): { preferences: AvailabilityPreferences; profiles: AvailabilityProfiles } {
  const chosen = profiles.otherProfiles.find((profile) => profile.id === id);
  if (!chosen) return { preferences, profiles };
  const previous: AvailabilityProfile = {
    ...profiles.defaultProfile,
    workingWindows: preferences.workingWindows,
    defaultDurationMinutes: preferences.defaultDurationMinutes,
    driveTimeBufferMinutes: preferences.driveTimeBufferMinutes,
  };
  return {
    preferences: {
      ...preferences,
      workingWindows: chosen.workingWindows,
      defaultDurationMinutes: chosen.defaultDurationMinutes,
      driveTimeBufferMinutes: chosen.driveTimeBufferMinutes,
    },
    profiles: {
      ...profiles,
      defaultProfile: { id: chosen.id, name: chosen.name },
      otherProfiles: profiles.otherProfiles.map((profile) => profile.id === id ? previous : profile),
      accountDefaults: Object.fromEntries(Object.entries(profiles.accountDefaults).filter(([, profileId]) => profileId !== id)),
    },
  };
}

/** Why a name can't be used, or null when it can. */
export function availabilityProfileNameError(profiles: AvailabilityProfiles, id: string, name: string): string | null {
  const trimmed = name.trim();
  if (!trimmed) return "Give the profile a name.";
  if (trimmed.length > MAX_AVAILABILITY_PROFILE_NAME_CHARS) return `Use ${MAX_AVAILABILITY_PROFILE_NAME_CHARS} characters or fewer.`;
  const clash = [profiles.defaultProfile, ...profiles.otherProfiles]
    .some((profile) => profile.id !== id && profile.name.toLowerCase() === trimmed.toLowerCase());
  return clash ? `Another profile is already called ${trimmed}.` : null;
}

/** An account's name as Settings shows it, with its address when that differs. */
export function availabilityAccountIdentity(account: Account): { name: string; address: string | null } {
  const name = account.nickname?.trim() || account.displayName?.trim() || account.email;
  return { name, address: name === account.email ? null : account.email };
}

function accountLabel(account: Account): string {
  const { name, address } = availabilityAccountIdentity(account);
  return address ? `${name} (${address})` : name;
}

export function meetingTypeLabel(type: MeetingType): string {
  return type[0].toUpperCase() + type.slice(1);
}

const SHORT_WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
// Summaries read Monday first, as a working week does.
const SUMMARY_WEEKDAY_ORDER = [1, 2, 3, 4, 5, 6, 0];
const MAX_SUMMARY_GROUPS = 2;

function summaryClock(value: string): string {
  const minutes = clockMinutes(value);
  return formatClockTime(new Date(2000, 0, 1, Math.floor(minutes / 60), minutes % 60));
}

function summaryDays(weekdays: number[]): string {
  const runs: number[][] = [];
  for (const weekday of weekdays) {
    const run = runs.at(-1);
    const position = SUMMARY_WEEKDAY_ORDER.indexOf(weekday);
    if (run && SUMMARY_WEEKDAY_ORDER.indexOf(run.at(-1)!) === position - 1) run.push(weekday);
    else runs.push([weekday]);
  }
  return runs.map((run) => run.length > 2
    ? `${SHORT_WEEKDAYS[run[0]]}–${SHORT_WEEKDAYS[run.at(-1)!]}`
    : run.map((weekday) => SHORT_WEEKDAYS[weekday]).join(", ")).join(", ");
}

/**
 * A one-line description of a profile's hours, such as "Mon–Fri 9:00 am–5:00 pm":
 * days with the same hours share a group, and groups past the first two are counted.
 */
export function summarizeAvailableHours(windows: AvailabilityWindow[]): string {
  const groups = new Map<string, number[]>();
  for (const weekday of SUMMARY_WEEKDAY_ORDER) {
    const hours = windows
      .filter((window) => window.weekday === weekday)
      .sort((a, b) => a.start.localeCompare(b.start))
      .map((window) => `${summaryClock(window.start)}–${summaryClock(window.end)}`)
      .join(", ");
    if (!hours) continue;
    groups.set(hours, [...(groups.get(hours) ?? []), weekday]);
  }
  if (groups.size === 0) return "No available hours";
  const parts = [...groups].map(([hours, weekdays]) => `${summaryDays(weekdays)} ${hours}`);
  const shown = parts.slice(0, MAX_SUMMARY_GROUPS);
  if (parts.length > shown.length) shown.push(`+${parts.length - shown.length} more`);
  return shown.join(" · ");
}

/** The profiles page: each profile's hours, with the settings every profile shares kept apart. */
export function AvailabilityProfilesSettings({
  preferences,
  onChange,
  profiles = DEFAULT_AVAILABILITY_PROFILES,
  onProfilesChange,
  accounts = [],
  selectedProfileId,
  onSelectProfile,
}: {
  /** The shared timezone and slot increment, with the default profile's hours. */
  preferences: AvailabilityPreferences;
  onChange(value: AvailabilityPreferences): void;
  profiles?: AvailabilityProfiles;
  /** Saves profiles, with the shared preferences when both change together. */
  onProfilesChange?(value: AvailabilityProfiles, preferences?: AvailabilityPreferences): void;
  accounts?: Account[];
  /** The profile being edited, when Settings keeps track of it across pages. */
  selectedProfileId?: string;
  onSelectProfile?(id: string): void;
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

  const sharedHintId = useId();
  const profilesHintId = useId();
  const editorHeadingId = useId();
  const sharedHeadingId = useId();
  const allProfiles = [profiles.defaultProfile, ...profiles.otherProfiles];
  const [ownSelectedId, setOwnSelectedId] = useState(profiles.defaultProfile.id);
  const selectedId = selectedProfileId ?? ownSelectedId;
  const setSelectedId = (id: string) => {
    setOwnSelectedId(id);
    onSelectProfile?.(id);
  };
  // A sync or Make Default can remove the profile being edited.
  const selected = allProfiles.find((profile) => profile.id === selectedId) ?? profiles.defaultProfile;
  const selectedIsDefault = selected.id === profiles.defaultProfile.id;
  const editing = availabilityWithProfile(preferences, profiles, selected.id);
  const [nameDraft, setNameDraft] = useState(selected.name);
  const [nameError, setNameError] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  useEffect(() => {
    setNameDraft(selected.name);
    setNameError(null);
    setDeleting(false);
  }, [selected.id, selected.name]);

  const updateHours = (patch: Partial<ProfileHours>) => {
    if (selectedIsDefault) {
      onChange({ ...preferences, ...patch });
      return;
    }
    onProfilesChange?.({
      ...profiles,
      otherProfiles: profiles.otherProfiles.map((profile) => profile.id === selected.id ? { ...profile, ...patch } : profile),
    });
  };
  const commitName = (value: string) => {
    const error = availabilityProfileNameError(profiles, selected.id, value);
    setNameError(error);
    const name = value.trim();
    if (error || name === selected.name) return;
    onProfilesChange?.(selectedIsDefault
      ? { ...profiles, defaultProfile: { ...profiles.defaultProfile, name } }
      : { ...profiles, otherProfiles: profiles.otherProfiles.map((profile) => profile.id === selected.id ? { ...profile, name } : profile) });
  };
  const addProfile = () => {
    const id = crypto.randomUUID();
    onProfilesChange?.(addAvailabilityProfile(preferences, profiles, id));
    setSelectedId(id);
  };
  const usedBy = accounts.filter((account) => availabilityProfileId(profiles, account.email) === selected.id);
  const usedByTypes = MEETING_TYPES.filter((type) => profiles.meetingTypeProfiles?.[type] === selected.id);
  const usage = [
    ...usedBy.map((account) => availabilityAccountIdentity(account).name),
    ...usedByTypes.map((type) => `${meetingTypeLabel(type)} invitations`),
  ];
  const canAddProfile = Boolean(onProfilesChange) && allProfiles.length < MAX_AVAILABILITY_PROFILES;

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
    updateHours({ workingWindows: editing.workingWindows.map((window) => window === target ? { ...window, ...patch } : window) });
  };
  const addWindow = (window: AvailabilityWindow) => updateHours({ workingWindows: [...editing.workingWindows, window] });
  const removeWindows = (shouldRemove: (window: AvailabilityWindow) => boolean) => updateHours({
    workingWindows: editing.workingWindows.filter((window) => !shouldRemove(window)),
  });
  return (
    <section className="settings-section availability-settings" aria-label="Profiles">
      <div className="settings-section-heading-row">
        <h3>Profiles</h3>
        {onProfilesChange ? <span className="settings-section-heading-actions">
          <button className="btn btn-sm" type="button" disabled={!canAddProfile} onClick={addProfile}><Plus size={ICON_SIZE.sm} />Add Profile</button>
        </span> : null}
      </div>
      <p className="settings-hint" id={profilesHintId}>Each profile sets when you’re willing to meet, with its own meeting length and drive time. Accounts and meeting types choose which profile they use. Every calendar used for availability still counts as busy, whichever profile is in use.</p>
      <div className="availability-profile-list" role="group" aria-label="Profiles" aria-describedby={profilesHintId}>
        {allProfiles.map((profile) => {
          const isDefault = profile.id === profiles.defaultProfile.id;
          const hours = availabilityWithProfile(preferences, profiles, profile.id).workingWindows;
          const summaryId = `${profilesHintId}-${profile.id}`;
          const badgeId = `${summaryId}-default`;
          return (
            <button
              key={profile.id}
              type="button"
              className="availability-profile-item"
              aria-pressed={profile.id === selected.id}
              aria-label={profile.name}
              aria-describedby={isDefault ? `${badgeId} ${summaryId}` : summaryId}
              onClick={() => setSelectedId(profile.id)}
            >
              <span className="availability-profile-item-name">
                <strong>{profile.name}</strong>
                {isDefault ? <span className="availability-default-badge" id={badgeId}>Default</span> : null}
              </span>
              <span className="availability-profile-item-summary" id={summaryId}>{summarizeAvailableHours(hours)}</span>
            </button>
          );
        })}
      </div>
      <section className="availability-profile-editor" aria-labelledby={editorHeadingId}>
        <h3 id={editorHeadingId}>{selected.name}</h3>
        <label className="settings-field">
          <span>Profile Name</span>
          <input
            value={nameDraft}
            maxLength={MAX_AVAILABILITY_PROFILE_NAME_CHARS}
            aria-invalid={nameError ? "true" : undefined}
            onChange={(event) => {
              setNameDraft(event.target.value);
              setNameError(null);
            }}
            onBlur={(event) => commitName(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                commitName(event.currentTarget.value);
              }
            }}
          />
        </label>
        {nameError ? <p className="form-error" role="alert">{nameError}</p> : null}
        <p className="settings-hint">{usage.length ? `Used by ${usage.join(", ")}.` : "No account or meeting type uses this profile yet."}</p>
        {selectedIsDefault ? <p className="settings-hint">The default profile is used when an account has no profile of its own.</p> : (
          <div className="settings-row">
            <button className="btn" type="button" onClick={() => {
              const next = makeDefaultAvailabilityProfile(preferences, profiles, selected.id);
              onProfilesChange?.(next.profiles, next.preferences);
            }}>Make Default</button>
            <button className="btn btn-danger" type="button" onClick={() => setDeleting(true)}>Delete Profile</button>
          </div>
        )}
        {deleting && !selectedIsDefault ? (
          <InlineConfirm
            ariaLabel={`Delete ${selected.name} confirmation`}
            cancelLabel="Cancel"
            onCancel={() => setDeleting(false)}
            actions={[{
              label: "Delete Profile",
              className: "btn-danger",
              onClick: () => {
                setDeleting(false);
                setSelectedId(profiles.defaultProfile.id);
                onProfilesChange?.(deleteAvailabilityProfile(profiles, selected.id));
              },
            }]}
          >
            <strong>Delete {selected.name}?</strong><br />
            {usedBy.length
              ? `${usedBy.map(accountLabel).join(", ")} will use ${profiles.defaultProfile.name}.`
              : "No account uses it."}
          </InlineConfirm>
        ) : null}
        <div className="settings-section-heading-row">
          <h4>Available Hours</h4>
          <span className="settings-section-heading-actions">
            <button className="btn btn-sm"
              type="button"
              onClick={() => {
                const mondayWindows = editing.workingWindows.filter((window) => window.weekday === 1);
                const monday = mondayWindows.length > 0 ? mondayWindows : [{ weekday: 1, start: "09:00", end: "17:00" }];
                const weekends = editing.workingWindows.filter((window) => window.weekday === 0 || window.weekday === 6);
                updateHours({
                  workingWindows: [
                    ...weekends,
                    ...[1, 2, 3, 4, 5].flatMap((weekday) => monday.map(({ start, end }) => ({ weekday, start, end }))),
                  ].sort((a, b) => a.weekday - b.weekday),
                });
              }}
            >
              Copy Monday to weekdays
            </button>
            <button className="btn btn-sm" type="button" onClick={() => updateHours({ workingWindows: [] })}>Clear</button>
          </span>
        </div>
        <div className="availability-windows">
          {weekdayLabels.map((label, weekday) => {
            const dayWindows = editing.workingWindows.filter((candidate) => candidate.weekday === weekday);
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
        <h4>Meetings</h4>
        <label className="settings-field settings-field-inline settings-field-fixed"><span>Default Duration</span><select value={editing.defaultDurationMinutes} onChange={(event) => updateHours({ defaultDurationMinutes: Number(event.target.value) })}>{DEFAULT_MEETING_DURATION_OPTIONS.map((value) => <option key={value} value={value}>{value} Minutes</option>)}</select></label>
        <label className="settings-field settings-field-inline settings-field-fixed">
          <span>Drive Time Buffer</span>
          <select value={editing.driveTimeBufferMinutes} onChange={(event) => updateHours({ driveTimeBufferMinutes: Number(event.target.value) })}>
            {[...new Set([...DRIVE_TIME_BUFFER_OPTIONS, editing.driveTimeBufferMinutes])].sort((a, b) => a - b).map((value) => <option key={value} value={value}>{value === 0 ? "None" : `${value} Minutes`}</option>)}
          </select>
        </label>
        <p className="settings-hint">Reserve this much time before and after timed calendar events with a location when checking or suggesting meeting times.</p>
      </section>
      <section className="availability-shared-settings" aria-labelledby={sharedHeadingId}>
        <h3 id={sharedHeadingId}>Shared Settings</h3>
        <p className="settings-hint" id={sharedHintId}>Shared by every profile.</p>
        <label className="settings-field">
          <span>Timezone</span>
          <input
            list="availability-timezones"
            value={timeZoneDraft}
            aria-label="Availability Timezone"
            aria-describedby={sharedHintId}
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
        <label className="settings-field settings-field-inline settings-field-fixed"><span>Slot Increment</span><select value={preferences.slotIncrementMinutes} aria-describedby={sharedHintId} onChange={(event) => onChange({ ...preferences, slotIncrementMinutes: Number(event.target.value) })}>{[5, 10, 15, 30, 60].map((value) => <option key={value} value={value}>{value} Minutes</option>)}</select></label>
      </section>
    </section>
  );
}
