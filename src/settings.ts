export type FontFamily = string;

const FONT_FAMILY_KEY = "dispatch.settings.fontFamily";
const MAX_FONT_FAMILY_LENGTH = 200;
const LEGACY_FONT_FAMILIES: Record<string, FontFamily> = {
  serif: "Georgia",
  mono: "Menlo",
  "avenir-next": "Avenir Next",
  "helvetica-neue": "Helvetica Neue",
  arial: "Arial",
  georgia: "Georgia",
  "times-new-roman": "Times New Roman",
  verdana: "Verdana",
  menlo: "Menlo",
};

export const DEFAULT_FONT_FAMILY: FontFamily = "system";

export const SYSTEM_FONT_STACK =
  'ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';

function validFontFamily(value: string | null): value is FontFamily {
  if (!value) return false;
  const trimmed = value.trim();
  return trimmed.length > 0
    && trimmed.length <= MAX_FONT_FAMILY_LENGTH
    && !/[\u0000-\u001f\u007f]/.test(trimmed);
}

function quoteCssString(value: string): string {
  return `"${value.replaceAll("\\", "\\\\").replaceAll('"', '\\"')}"`;
}

export function fontFamilyStack(value: FontFamily): string {
  return value === DEFAULT_FONT_FAMILY
    ? SYSTEM_FONT_STACK
    : `${quoteCssString(value)}, ${SYSTEM_FONT_STACK}`;
}

const AUTO_READ_DELAY_SECONDS_KEY = "dispatch.settings.autoReadDelaySeconds";

const LOAD_REMOTE_IMAGES_KEY = "dispatch.settings.loadRemoteImages";

export const DEFAULT_LOAD_REMOTE_IMAGES = false;

export const DEFAULT_AUTO_READ_DELAY_SECONDS = 2;
export const MIN_AUTO_READ_DELAY_SECONDS = 0;
export const MAX_AUTO_READ_DELAY_SECONDS = 60;

export function clampAutoReadDelaySeconds(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_AUTO_READ_DELAY_SECONDS;
  return Math.min(
    MAX_AUTO_READ_DELAY_SECONDS,
    Math.max(MIN_AUTO_READ_DELAY_SECONDS, Math.round(value)),
  );
}

export function readAutoReadDelaySeconds(): number {
  try {
    const saved = localStorage.getItem(AUTO_READ_DELAY_SECONDS_KEY);
    if (saved !== null && saved.trim() !== "") {
      return clampAutoReadDelaySeconds(Number(saved));
    }
  } catch {
    // A blocked storage backend should not prevent the app from opening.
  }
  return DEFAULT_AUTO_READ_DELAY_SECONDS;
}

export function saveAutoReadDelaySeconds(value: number): number {
  const next = clampAutoReadDelaySeconds(value);
  try {
    localStorage.setItem(AUTO_READ_DELAY_SECONDS_KEY, String(next));
  } catch {
    // The preference still applies for this session when storage is unavailable.
  }
  return next;
}

export function readLoadRemoteImages(): boolean {
  try {
    return localStorage.getItem(LOAD_REMOTE_IMAGES_KEY) === "true";
  } catch {
    // A blocked storage backend should not prevent the app from opening.
  }
  return DEFAULT_LOAD_REMOTE_IMAGES;
}

export function saveLoadRemoteImages(value: boolean): boolean {
  try {
    localStorage.setItem(LOAD_REMOTE_IMAGES_KEY, String(value));
  } catch {
    // The preference still applies for this session when storage is unavailable.
  }
  return value;
}

export function readFontFamily(): FontFamily {
  try {
    const saved = localStorage.getItem(FONT_FAMILY_KEY);
    // Preserve preferences saved by the earlier generic and curated selectors.
    if (saved && LEGACY_FONT_FAMILIES[saved]) return LEGACY_FONT_FAMILIES[saved];
    if (validFontFamily(saved)) return saved.trim();
  } catch {
    // A blocked storage backend should not prevent the app from opening.
  }
  return DEFAULT_FONT_FAMILY;
}

export function applyFontFamily(value: FontFamily): void {
  document.documentElement.style.setProperty("--font-family", fontFamilyStack(value));
}

export function saveFontFamily(value: FontFamily): FontFamily {
  const next = validFontFamily(value) ? value.trim() : DEFAULT_FONT_FAMILY;
  applyFontFamily(next);
  try {
    localStorage.setItem(FONT_FAMILY_KEY, next);
  } catch {
    // The preference still applies for this session when storage is unavailable.
  }
  return next;
}
