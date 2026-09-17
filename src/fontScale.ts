export const FONT_SCALE_KEY = "threestrands.fontScale";
export const DEFAULT_FONT_SCALE = 100;
export const MIN_FONT_SCALE = 80;
export const MAX_FONT_SCALE = 140;
export const FONT_SCALE_STEP = 10;

export function clampFontScale(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_FONT_SCALE;
  return Math.min(MAX_FONT_SCALE, Math.max(MIN_FONT_SCALE, value));
}

export function readFontScale(): number {
  const saved = Number(localStorage.getItem(FONT_SCALE_KEY));
  return saved ? clampFontScale(saved) : DEFAULT_FONT_SCALE;
}

export function saveFontScale(value: number): number {
  const clamped = clampFontScale(value);
  localStorage.setItem(FONT_SCALE_KEY, String(clamped));
  return clamped;
}

export function applyFontScale(value: number): void {
  document.documentElement.style.setProperty(
    "--font-scale",
    String(clampFontScale(value) / 100),
  );
}

export function changeFontScale(value: number, direction: 1 | -1): number {
  return clampFontScale(value + direction * FONT_SCALE_STEP);
}
