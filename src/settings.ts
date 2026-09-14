export type FontFamily =
  | "system"
  | "avenir-next"
  | "helvetica-neue"
  | "arial"
  | "georgia"
  | "times-new-roman"
  | "verdana"
  | "menlo";

const FONT_FAMILY_KEY = "dispatch.settings.fontFamily";

export const DEFAULT_FONT_FAMILY: FontFamily = "system";

export const FONT_FAMILY_STACKS: Record<FontFamily, string> = {
  system: 'ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif',
  "avenir-next": '"Avenir Next", Avenir, ui-sans-serif, sans-serif',
  "helvetica-neue": '"Helvetica Neue", Helvetica, Arial, sans-serif',
  arial: 'Arial, "Helvetica Neue", sans-serif',
  georgia: 'Georgia, "Times New Roman", serif',
  "times-new-roman": '"Times New Roman", Times, serif',
  verdana: 'Verdana, Geneva, sans-serif',
  menlo: 'Menlo, Monaco, Consolas, ui-monospace, monospace',
};

export const FONT_FAMILY_OPTIONS: { value: FontFamily; label: string }[] = [
  { value: "system", label: "System default" },
  { value: "avenir-next", label: "Avenir Next" },
  { value: "helvetica-neue", label: "Helvetica Neue" },
  { value: "arial", label: "Arial" },
  { value: "georgia", label: "Georgia" },
  { value: "times-new-roman", label: "Times New Roman" },
  { value: "verdana", label: "Verdana" },
  { value: "menlo", label: "Menlo" },
];

const fontFamilies = new Set<FontFamily>(
  FONT_FAMILY_OPTIONS.map(({ value }) => value),
);

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
    // Preserve preferences saved by the earlier generic family selector.
    if (saved === "serif") return "georgia";
    if (saved === "mono") return "menlo";
    if (fontFamilies.has(saved as FontFamily)) return saved as FontFamily;
  } catch {
    // A blocked storage backend should not prevent the app from opening.
  }
  return DEFAULT_FONT_FAMILY;
}

export function applyFontFamily(value: FontFamily): void {
  document.documentElement.style.setProperty("--font-family", FONT_FAMILY_STACKS[value]);
}

export function saveFontFamily(value: FontFamily): FontFamily {
  applyFontFamily(value);
  try {
    localStorage.setItem(FONT_FAMILY_KEY, value);
  } catch {
    // The preference still applies for this session when storage is unavailable.
  }
  return value;
}
