import { useSyncExternalStore, type CSSProperties } from "react";

/**
 * The fixed palette a calendar's meetings can be tinted with. Only these ids
 * are ever stored, so a hand-edited or stale storage value can never put an
 * arbitrary CSS string into an inline style.
 */
export const CALENDAR_COLORS = [
  { id: "red", label: "Red", value: "#d93f3f" },
  { id: "coral", label: "Coral", value: "#e8776b" },
  { id: "orange", label: "Orange", value: "#ee7a2f" },
  { id: "amber", label: "Amber", value: "#e9a623" },
  { id: "yellow", label: "Yellow", value: "#d8c32a" },
  { id: "lime", label: "Lime", value: "#94b83a" },
  { id: "green", label: "Green", value: "#3a9d5d" },
  { id: "teal", label: "Teal", value: "#1f9a8f" },
  { id: "cyan", label: "Cyan", value: "#22a3c4" },
  { id: "sky", label: "Sky", value: "#4a9be8" },
  { id: "blue", label: "Blue", value: "#3d6fd9" },
  { id: "indigo", label: "Indigo", value: "#5c5fc8" },
  { id: "purple", label: "Purple", value: "#8a55c9" },
  { id: "pink", label: "Pink", value: "#d454a0" },
  { id: "brown", label: "Brown", value: "#8d6748" },
  { id: "gray", label: "Gray", value: "#7a7f87" },
] as const;

export type CalendarColorId = (typeof CALENDAR_COLORS)[number]["id"];
/** Palette id by account email, then calendar id. */
export type CalendarColorMap = Record<string, Record<string, CalendarColorId>>;

export const CALENDAR_COLORS_KEY = "threestrands.calendarColors";

const colorIds = new Set<string>(CALENDAR_COLORS.map((color) => color.id));
const listeners = new Set<() => void>();
let snapshot: CalendarColorMap | null = null;

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function readCalendarColors(): CalendarColorMap {
  const colors: CalendarColorMap = {};
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(CALENDAR_COLORS_KEY) ?? "{}");
    if (!isPlainObject(parsed)) return colors;
    for (const [accountId, calendars] of Object.entries(parsed)) {
      if (!isPlainObject(calendars)) continue;
      for (const [calendarId, colorId] of Object.entries(calendars)) {
        if (typeof colorId !== "string" || !colorIds.has(colorId)) continue;
        (colors[accountId] ??= {})[calendarId] = colorId as CalendarColorId;
      }
    }
  } catch {
    // Unreadable or blocked storage leaves every calendar on the default color.
  }
  return colors;
}

export function setCalendarColor(accountId: string, calendarId: string, colorId: CalendarColorId) {
  if (!colorIds.has(colorId)) return;
  const current = snapshot ?? readCalendarColors();
  snapshot = { ...current, [accountId]: { ...current[accountId], [calendarId]: colorId } };
  try {
    localStorage.setItem(CALENDAR_COLORS_KEY, JSON.stringify(snapshot));
  } catch {
    // The color still applies for this session when storage is unavailable.
  }
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

function getSnapshot() {
  return snapshot ??= readCalendarColors();
}

/** Re-reads storage on next access; for tests that seed or clear it directly. */
export function resetCalendarColorsForTests() {
  snapshot = null;
  listeners.forEach((listener) => listener());
}

/** Calendar colors shared by every surface, so a change in one shows everywhere. */
export function useCalendarColors(): CalendarColorMap {
  return useSyncExternalStore(subscribe, getSnapshot);
}

export function calendarColorId(colors: CalendarColorMap, accountId: string, calendarId?: string): CalendarColorId | undefined {
  return calendarId ? colors[accountId]?.[calendarId] : undefined;
}

/** Inline style that tints an event or calendar row; undefined keeps the accent color. */
export function calendarColorStyle(colors: CalendarColorMap, accountId: string, calendarId?: string): CSSProperties | undefined {
  const id = calendarColorId(colors, accountId, calendarId);
  const color = CALENDAR_COLORS.find((candidate) => candidate.id === id);
  return color ? { "--calendar-color": color.value } as CSSProperties : undefined;
}
