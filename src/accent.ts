export const ACCENTS = ["purple", "blue", "teal", "green", "amber", "rose", "graphite"] as const;

export type Accent = (typeof ACCENTS)[number];

export const DEFAULT_ACCENT: Accent = "purple";

const storageKey = "threestrands.accent";

export function readAccent(): Accent {
  try {
    const saved = localStorage.getItem(storageKey);
    if (saved && (ACCENTS as readonly string[]).includes(saved)) return saved as Accent;
  } catch {
    // A blocked storage backend should not prevent the app from opening.
  }
  return DEFAULT_ACCENT;
}

export function applyAccent(accent: Accent) {
  document.documentElement.dataset.accent = accent;
}

export function saveAccent(accent: Accent) {
  applyAccent(accent);
  try {
    localStorage.setItem(storageKey, accent);
  } catch {
    // The picker still works for this session when storage is unavailable.
  }
}
