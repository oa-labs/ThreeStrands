import type { EmailTextColors } from "./emailRenderingPolicy";
import { useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  applyFontScale,
  changeFontScale,
  readFontScale,
  saveFontScale,
} from "./fontScale";
import {
  applyFontFamily,
  readAutoReadDelaySeconds,
  readFontFamily,
  readEmailMinimumFontSize,
  readEmailTextColors,
  readEmailZoom,
  readLoadRemoteImages,
  availabilityProfileId,
  availabilityWithProfile,
  readAvailabilityPreferences,
  readAvailabilityProfiles,
  saveAvailabilityPreferences,
  saveAvailabilityProfiles,
  saveAutoReadDelaySeconds,
  saveFontFamily,
  saveEmailMinimumFontSize,
  saveEmailTextColors,
  saveEmailZoom,
  saveLoadRemoteImages,
  type FontFamily,
} from "./settings";
import {
  applyAccent,
  readAccent,
  saveAccent,
  type Accent,
} from "./accent";
import {
  applyTheme,
  effectiveTheme,
  readTheme,
  saveTheme,
  type Theme,
} from "./theme";
import { pullSyncedPreferences, queuePortablePreferences } from "./syncedPreferences";
import type { AvailabilityPreferences, AvailabilityProfiles } from "./domain";

/**
 * Owns the persisted preferences that affect the whole application.
 *
 * Keeping persistence and document-level side effects here gives callers one
 * operation per preference instead of requiring every settings surface to
 * coordinate local state, storage, and CSS independently.
 */
export function useAppPreferences() {
  const [theme, setThemeState] = useState(readTheme);
  const [accent, setAccentState] = useState(readAccent);
  const [fontScale, setFontScaleState] = useState(readFontScale);
  const [emailZoom, setEmailZoomState] = useState(readEmailZoom);
  const [emailMinimumFontSize, setEmailMinimumFontSizeState] = useState(readEmailMinimumFontSize);
  const [emailTextColors, setEmailTextColorsState] = useState(readEmailTextColors);
  const [fontFamily, setFontFamilyState] = useState(readFontFamily);
  const [autoReadDelaySeconds, setAutoReadDelayState] = useState(readAutoReadDelaySeconds);
  const [loadRemoteImages, setLoadRemoteImagesState] = useState(readLoadRemoteImages);
  const [availabilityPreferences, setAvailabilityPreferencesState] = useState(readAvailabilityPreferences);
  const [availabilityProfiles, setAvailabilityProfilesState] = useState(readAvailabilityProfiles);

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    const applySynced = () => {
      void pullSyncedPreferences().then((changed) => {
        if (changed) {
          setThemeState(readTheme());
          setFontScaleState(readFontScale());
          setFontFamilyState(readFontFamily());
          setEmailMinimumFontSizeState(readEmailMinimumFontSize());
          setAutoReadDelayState(readAutoReadDelaySeconds());
          setLoadRemoteImagesState(readLoadRemoteImages());
          // Keep the same object when availability didn't change: meeting
          // cards re-check the calendar whenever it is replaced.
          const synced = readAvailabilityPreferences();
          setAvailabilityPreferencesState((current) => JSON.stringify(current) === JSON.stringify(synced) ? current : synced);
          const syncedProfiles = readAvailabilityProfiles();
          setAvailabilityProfilesState((current) => JSON.stringify(current) === JSON.stringify(syncedProfiles) ? current : syncedProfiles);
        }
        // Publish the complete portable preference record before adding
        // per-device names to it, so older clients still materialize a
        // complete preferences object when they receive name updates. The
        // native command records only changed fields, so this is safe after
        // every sync-status refresh and also retries once sync is enabled.
        queuePortablePreferences();
      });
    };
    applySynced();
    let stop: (() => void) | undefined;
    void listen("replicated-sync-status", applySynced).then((unlisten) => { stop = unlisten; });
    return () => stop?.();
  }, []);

  useEffect(() => applyTheme(theme), [theme]);
  useEffect(() => applyAccent(accent), [accent]);
  useEffect(() => applyFontScale(fontScale), [fontScale]);
  useEffect(() => applyFontFamily(fontFamily), [fontFamily]);
  useEffect(() => {
    if (theme !== "system") return;
    const query = window.matchMedia?.("(prefers-color-scheme: light)");
    if (!query) return;
    const handleChange = () => applyTheme("system");
    query.addEventListener("change", handleChange);
    return () => query.removeEventListener("change", handleChange);
  }, [theme]);

  const setTheme = useCallback((next: Theme) => {
    saveTheme(next);
    setThemeState(next);
  }, []);
  const setAccent = useCallback((next: Accent) => {
    saveAccent(next);
    setAccentState(next);
  }, []);
  const toggleTheme = useCallback(() => {
    setThemeState((current) => {
      const next = effectiveTheme(current) === "dark" ? "light" : "dark";
      saveTheme(next);
      return next;
    });
  }, []);
  const setFontScale = useCallback((value: number) => {
    setFontScaleState(saveFontScale(value));
    queuePortablePreferences();
  }, []);
  const adjustFontScale = useCallback((direction: 1 | -1) => {
    setFontScaleState((current) => saveFontScale(changeFontScale(current, direction)));
    queuePortablePreferences();
  }, []);
  const setEmailZoom = useCallback((value: number) => {
    setEmailZoomState(saveEmailZoom(value));
  }, []);
  const setEmailTextColors = useCallback((value: EmailTextColors) => {
    setEmailTextColorsState(saveEmailTextColors(value));
  }, []);
  const setEmailMinimumFontSize = useCallback((value: number) => {
    setEmailMinimumFontSizeState(saveEmailMinimumFontSize(value));
    queuePortablePreferences();
  }, []);
  const setFontFamily = useCallback((value: FontFamily) => {
    setFontFamilyState(saveFontFamily(value));
    queuePortablePreferences();
  }, []);
  const setAutoReadDelaySeconds = useCallback((value: number) => {
    setAutoReadDelayState(saveAutoReadDelaySeconds(value));
    queuePortablePreferences();
  }, []);
  const setLoadRemoteImages = useCallback((value: boolean) => {
    setLoadRemoteImagesState(saveLoadRemoteImages(value));
    queuePortablePreferences();
  }, []);
  const setAvailabilityPreferences = useCallback((value: Parameters<typeof saveAvailabilityPreferences>[0]) => {
    setAvailabilityPreferencesState(saveAvailabilityPreferences(value));
    queuePortablePreferences();
  }, []);
  /** Saves profiles, and the default profile's hours when they change together (Make Default). */
  const setAvailabilityProfiles = useCallback((value: AvailabilityProfiles, preferences?: AvailabilityPreferences) => {
    if (preferences) setAvailabilityPreferencesState(saveAvailabilityPreferences(preferences));
    setAvailabilityProfilesState(saveAvailabilityProfiles(value));
    queuePortablePreferences();
  }, []);
  // One object per profile, replaced only when settings change: meeting
  // cards re-check the calendar whenever their preferences are replaced.
  const preferencesByProfile = useMemo(() => new Map([
    [availabilityProfiles.defaultProfile.id, availabilityPreferences],
    ...availabilityProfiles.otherProfiles.map((profile) => [
      profile.id, availabilityWithProfile(availabilityPreferences, availabilityProfiles, profile.id),
    ] as const),
  ]), [availabilityPreferences, availabilityProfiles]);
  /** The profile in use: one chosen for a conversation while it exists, else the account's default profile. */
  const availabilityProfileFor = useCallback((accountId?: string | null, chosenId?: string | null): string =>
    chosenId && preferencesByProfile.has(chosenId) ? chosenId : availabilityProfileId(availabilityProfiles, accountId),
  [availabilityProfiles, preferencesByProfile]);
  /** The preferences for that profile; no account uses the default profile. */
  const availabilityFor = useCallback((accountId?: string | null, chosenId?: string | null): AvailabilityPreferences =>
    preferencesByProfile.get(availabilityProfileFor(accountId, chosenId)) ?? availabilityPreferences,
  [availabilityPreferences, availabilityProfileFor, preferencesByProfile]);

  return {
    theme,
    effectiveTheme: effectiveTheme(theme),
    setTheme,
    toggleTheme,
    accent,
    setAccent,
    fontScale,
    setFontScale,
    adjustFontScale,
    fontFamily,
    setFontFamily,
    emailZoom,
    setEmailZoom,
    emailMinimumFontSize,
    setEmailMinimumFontSize,
    emailTextColors,
    setEmailTextColors,
    autoReadDelaySeconds,
    setAutoReadDelaySeconds,
    loadRemoteImages,
    setLoadRemoteImages,
    availabilityPreferences,
    setAvailabilityPreferences,
    availabilityProfiles,
    setAvailabilityProfiles,
    availabilityProfileFor,
    availabilityFor,
  };
}
