import { useCallback, useEffect, useState } from "react";
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
  readLoadRemoteImages,
  readAvailabilityPreferences,
  saveAvailabilityPreferences,
  saveAutoReadDelaySeconds,
  saveFontFamily,
  saveLoadRemoteImages,
  type FontFamily,
} from "./settings";
import {
  applyTheme,
  effectiveTheme,
  readTheme,
  saveTheme,
  type Theme,
} from "./theme";
import { pullSyncedPreferences, queuePortablePreferences } from "./syncedPreferences";

/**
 * Owns the persisted preferences that affect the whole application.
 *
 * Keeping persistence and document-level side effects here gives callers one
 * operation per preference instead of requiring every settings surface to
 * coordinate local state, storage, and CSS independently.
 */
export function useAppPreferences() {
  const [theme, setThemeState] = useState(readTheme);
  const [fontScale, setFontScaleState] = useState(readFontScale);
  const [fontFamily, setFontFamilyState] = useState(readFontFamily);
  const [autoReadDelaySeconds, setAutoReadDelayState] = useState(readAutoReadDelaySeconds);
  const [loadRemoteImages, setLoadRemoteImagesState] = useState(readLoadRemoteImages);
  const [availabilityPreferences, setAvailabilityPreferencesState] = useState(readAvailabilityPreferences);

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    const applySynced = () => {
      void pullSyncedPreferences().then((changed) => {
        if (!changed) return;
        setThemeState(readTheme());
        setFontScaleState(readFontScale());
        setFontFamilyState(readFontFamily());
        setAutoReadDelayState(readAutoReadDelaySeconds());
        setLoadRemoteImagesState(readLoadRemoteImages());
        setAvailabilityPreferencesState(readAvailabilityPreferences());
      });
    };
    applySynced();
    let stop: (() => void) | undefined;
    void listen("replicated-sync-status", applySynced).then((unlisten) => { stop = unlisten; });
    return () => stop?.();
  }, []);

  useEffect(() => applyTheme(theme), [theme]);
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
    queuePortablePreferences();
  }, []);
  const toggleTheme = useCallback(() => {
    setThemeState((current) => {
      const next = effectiveTheme(current) === "dark" ? "light" : "dark";
      saveTheme(next);
      queuePortablePreferences();
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

  return {
    theme,
    effectiveTheme: effectiveTheme(theme),
    setTheme,
    toggleTheme,
    fontScale,
    setFontScale,
    adjustFontScale,
    fontFamily,
    setFontFamily,
    autoReadDelaySeconds,
    setAutoReadDelaySeconds,
    loadRemoteImages,
    setLoadRemoteImages,
    availabilityPreferences,
    setAvailabilityPreferences,
  };
}
