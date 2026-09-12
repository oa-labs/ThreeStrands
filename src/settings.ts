export type FontFamily = "system" | "serif" | "mono";

const FONT_FAMILY_KEY = "dispatch.settings.fontFamily";

export const DEFAULT_FONT_FAMILY: FontFamily = "system";

const FONT_FAMILY_STACKS: Record<FontFamily, string> = {
  system: 'Inter, ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif',
  serif: 'Iowan Old Style, Palatino Linotype, "Georgia", serif',
  mono: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
};

export const FONT_FAMILY_OPTIONS: { value: FontFamily; label: string }[] = [
  { value: "system", label: "System default" },
  { value: "serif", label: "Serif" },
  { value: "mono", label: "Monospace" },
];

export function readFontFamily(): FontFamily {
  try {
    const saved = localStorage.getItem(FONT_FAMILY_KEY);
    if (saved === "system" || saved === "serif" || saved === "mono") return saved;
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
