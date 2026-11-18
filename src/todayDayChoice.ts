import { readLocalStorage, writeLocalStorage } from "./useLocalStorageState";

/**
 * The day picked with the schedule's arrows, kept on this device so leaving
 * Today and coming back shows the same day. It is device-only interface state:
 * never a synced preference and never part of the settings export.
 */
export const TODAY_DAY_KEY = "threestrands.today.shownDay";

export type DayChoice = {
  /** The local day (YYYY-MM-DD) the choice was made on; it lasts until that day ends. */
  chosenOn: string;
  /** The local day (YYYY-MM-DD) the schedule shows. */
  day: string;
};

const DAY = /^\d{4}-\d{2}-\d{2}$/;

/** The stored choice when it was made `today`; null otherwise, which means show today. Unreadable storage reads as null. */
export function readDayChoice(today: string): DayChoice | null {
  return readLocalStorage<DayChoice | null>(TODAY_DAY_KEY, (raw) => {
    const stored = raw ? (JSON.parse(raw) as Partial<DayChoice> | null) : null;
    return stored && stored.chosenOn === today && typeof stored.day === "string" && DAY.test(stored.day) ? { chosenOn: today, day: stored.day } : null;
  }, null);
}

export function writeDayChoice(choice: DayChoice): void {
  writeLocalStorage(TODAY_DAY_KEY, JSON.stringify(choice));
}

/** Choosing Today, or the day turning over, forgets the choice. */
export function clearDayChoice(): void {
  try { localStorage.removeItem(TODAY_DAY_KEY); } catch { /* Nothing stored is read as today. */ }
}

/** The choice that still applies on `today`: one made on an earlier day has lapsed. */
export function activeDayChoice(choice: DayChoice | null, today: string): DayChoice | null {
  return choice && choice.chosenOn === today && choice.day !== today ? choice : null;
}

/** A stored day as a local midnight, or null when it isn't a real date. */
export function localDay(value: string): Date | null {
  if (!DAY.test(value)) return null;
  const [year, month, day] = value.split("-").map(Number);
  const date = new Date(year, month - 1, day);
  return date.getFullYear() === year && date.getMonth() === month - 1 && date.getDate() === day ? date : null;
}
