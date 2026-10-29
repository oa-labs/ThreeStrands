import { useEffect, useId, useMemo, useState } from "react";
import { Plus, X } from "lucide-react";
import type { Account, AvailabilityPreferences, AvailabilityProfile, AvailabilityProfiles, AvailabilityWindow } from "./domain";
import { ICON_SIZE } from "./iconSizes";
import { InlineConfirm } from "./InlineConfirm";
import {
  DEFAULT_AVAILABILITY_PROFILES,
  DRIVE_TIME_BUFFER_OPTIONS,
  MAX_AVAILABILITY_PROFILE_NAME_CHARS,
  MAX_AVAILABILITY_PROFILES,
  MAX_WORKING_WINDOWS_PER_DAY,
  availabilityAccountKey,
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

function accountLabel(account: Account): string {
  return account.nickname ? `${account.nickname} (${account.email})` : account.email;
}

export function AvailabilitySettings({
  preferences,
  onChange,
  profiles = DEFAULT_AVAILABILITY_PROFILES,
  onProfilesChange,
  accounts = [],
}: {
  /** The shared timezone and slot increment, with the default profile's hours. */
  preferences: AvailabilityPreferences;
  onChange(value: AvailabilityPreferences): void;
  profiles?: AvailabilityProfiles;
  /** Saves profiles, with the shared preferences when both change together. */
  onProfilesChange?(value: AvailabilityProfiles, preferences?: AvailabilityPreferences): void;
  accounts?: Account[];
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
  const allProfiles = [profiles.defaultProfile, ...profiles.otherProfiles];
  const hasProfiles = profiles.otherProfiles.length > 0;
  const [selectedId, setSelectedId] = useState(profiles.defaultProfile.id);
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
  const canAddProfile = Boolean(onProfilesChange) && allProfiles.length < MAX_AVAILABILITY_PROFILES;
  const sharedHint = hasProfiles ? sharedHintId : undefined;

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
    <section className="settings-section" aria-label="Availability">
      {hasProfiles ? <>
        <div className="settings-section-heading-row">
          <h3>Profiles</h3>
          <span className="settings-section-heading-actions">
            <button className="btn btn-sm" type="button" disabled={!canAddProfile} onClick={addProfile}><Plus size={ICON_SIZE.sm} />Add Profile</button>
          </span>
        </div>
        <div className="segmented availability-profiles" role="group" aria-label="Profiles" aria-describedby={profilesHintId}>
          {allProfiles.map((profile) => (
            <button key={profile.id} type="button" className="segment" aria-pressed={profile.id === selected.id} onClick={() => setSelectedId(profile.id)}>{profile.name}</button>
          ))}
        </div>
        <p className="settings-hint" id={profilesHintId}>Each profile has its own working hours, meeting length, and drive time. Every calendar used for availability still counts as busy, whichever profile is in use.</p>
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
      </> : null}
      {hasProfiles ? <p className="settings-hint" id={sharedHintId}>Shared by every profile.</p> : null}
      <h3>Timezone</h3>
      <label className="settings-field">
        <span>Timezone</span>
        <input
          list="availability-timezones"
          value={timeZoneDraft}
          aria-label="Availability Timezone"
          aria-describedby={sharedHint}
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
      <h3>Meeting Defaults</h3>
      <label className="settings-field settings-field-inline settings-field-fixed"><span>Default Duration</span><select value={editing.defaultDurationMinutes} onChange={(event) => updateHours({ defaultDurationMinutes: Number(event.target.value) })}>{[15, 30, 45, 60, 90, 120].map((value) => <option key={value} value={value}>{value} Minutes</option>)}</select></label>
      <label className="settings-field settings-field-inline settings-field-fixed"><span>Slot Increment</span><select value={preferences.slotIncrementMinutes} aria-describedby={sharedHint} onChange={(event) => onChange({ ...preferences, slotIncrementMinutes: Number(event.target.value) })}>{[5, 10, 15, 30, 60].map((value) => <option key={value} value={value}>{value} Minutes</option>)}</select></label>
      <h3>Drive Time</h3>
      <label className="settings-field settings-field-inline settings-field-fixed">
        <span>Drive Time Buffer</span>
        <select value={editing.driveTimeBufferMinutes} onChange={(event) => updateHours({ driveTimeBufferMinutes: Number(event.target.value) })}>
          {[...new Set([...DRIVE_TIME_BUFFER_OPTIONS, editing.driveTimeBufferMinutes])].sort((a, b) => a - b).map((value) => <option key={value} value={value}>{value === 0 ? "None" : `${value} Minutes`}</option>)}
        </select>
      </label>
      <p className="settings-hint">Reserve this much time before and after timed calendar events with a location when checking or suggesting meeting times.</p>
      {hasProfiles && accounts.length > 0 ? <>
        <h3>Account Defaults</h3>
        <p className="settings-hint">Finding times for an account's email uses its profile.</p>
        {accounts.map((account) => (
          <label key={account.email} className="settings-field settings-field-inline">
            <span>{accountLabel(account)}</span>
            <select value={availabilityProfileId(profiles, account.email)} onChange={(event) => {
              const key = availabilityAccountKey(account.email);
              const { [key]: _previous, ...others } = profiles.accountDefaults;
              onProfilesChange?.({
                ...profiles,
                accountDefaults: event.target.value === profiles.defaultProfile.id ? others : { ...others, [key]: event.target.value },
              });
            }}>
              {allProfiles.map((profile) => <option key={profile.id} value={profile.id}>{profile.name}</option>)}
            </select>
          </label>
        ))}
      </> : null}
      {!hasProfiles && onProfilesChange ? <>
        <h3>Profiles</h3>
        <div className="settings-row">
          <button className="btn" type="button" onClick={addProfile}><Plus size={ICON_SIZE.sm} />Add Profile</button>
          <span className="settings-hint">Keep separate hours for another part of your life, such as personal or volunteer meetings, and choose which one each account uses.</span>
        </div>
      </> : null}
    </section>
  );
}
