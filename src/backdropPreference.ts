import { useSyncExternalStore } from "react";
import { readLocalStorage, writeLocalStorage } from "./useLocalStorageState";

/**
 * Settings → Appearance → Today Backdrop: an optional decorative band behind
 * Today's header. Like Start With, it is a device-only preference kept in
 * local storage; it is not part of the user preferences that sync or go into a
 * settings export.
 */
export const TODAY_BACKDROP_KEY = "threestrands.today.backdrop";

const listeners = new Set<() => void>();

/** Off unless the user turned it on; unreadable storage reads as off. */
export function readTodayBackdrop(): boolean {
  return readLocalStorage(TODAY_BACKDROP_KEY, (raw) => raw === "on", false);
}

export function writeTodayBackdrop(on: boolean): void {
  writeLocalStorage(TODAY_BACKDROP_KEY, on ? "on" : "off");
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  const onStorage = (event: StorageEvent) => { if (event.key === null || event.key === TODAY_BACKDROP_KEY) listener(); };
  window.addEventListener("storage", onStorage);
  return () => { listeners.delete(listener); window.removeEventListener("storage", onStorage); };
}

/** The preference, read again when Settings changes it. */
export function useTodayBackdrop(): boolean {
  return useSyncExternalStore(subscribe, readTodayBackdrop, () => false);
}

/** The hour the sun rises and sets in the daytime scene. */
export const SUNRISE_HOUR = 6;
export const SUNSET_HOUR = 18;

/**
 * How high the sun stands at `now`, from 0 (on the horizon, or below it
 * before sunrise and after sunset) to 1 (noon): a half sine over the day.
 */
export function sunHeight(now: Date): number {
  const hour = now.getHours() + now.getMinutes() / 60;
  if (hour <= SUNRISE_HOUR || hour >= SUNSET_HOUR) return 0;
  return Math.sin(Math.PI * (hour - SUNRISE_HOUR) / (SUNSET_HOUR - SUNRISE_HOUR));
}
