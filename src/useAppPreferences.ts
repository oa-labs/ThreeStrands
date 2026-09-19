import { useCallback, useEffect, useState } from "react";
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
  }, []);
  const adjustFontScale = useCallback((direction: 1 | -1) => {
    setFontScaleState((current) => saveFontScale(changeFontScale(current, direction)));
  }, []);
  const setFontFamily = useCallback((value: FontFamily) => {
    setFontFamilyState(saveFontFamily(value));
  }, []);
  const setAutoReadDelaySeconds = useCallback((value: number) => {
    setAutoReadDelayState(saveAutoReadDelaySeconds(value));
  }, []);
  const setLoadRemoteImages = useCallback((value: boolean) => {
    setLoadRemoteImagesState(saveLoadRemoteImages(value));
  }, []);
  const setAvailabilityPreferences = useCallback((value: Parameters<typeof saveAvailabilityPreferences>[0]) => {
    setAvailabilityPreferencesState(saveAvailabilityPreferences(value));
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
