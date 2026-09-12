export const composerSizeKey = "dispatch.composerSize";
export const composerPositionKey = "dispatch.composerPosition";
export const minimumComposerWidth = 480;
export const minimumComposerHeight = 380;

export type ComposerSize = { width: number; height: number };
export type ComposerPosition = { x: number; y: number };
export type ViewportSize = { width: number; height: number };

export function composerBounds(viewport: ViewportSize = { width: window.innerWidth, height: window.innerHeight }) {
  return {
    maxWidth: Math.max(320, viewport.width - 48),
    maxHeight: Math.max(320, viewport.height - 48),
  };
}

export function clampComposerSize(size: ComposerSize, viewport?: ViewportSize): ComposerSize {
  const { maxWidth, maxHeight } = composerBounds(viewport);
  return {
    width: Math.round(Math.max(Math.min(minimumComposerWidth, maxWidth), Math.min(maxWidth, size.width))),
    height: Math.round(Math.max(Math.min(minimumComposerHeight, maxHeight), Math.min(maxHeight, size.height))),
  };
}

export function clampComposerPosition(position: ComposerPosition, size: ComposerSize, viewport: ViewportSize): ComposerPosition {
  const maxX = Math.max(0, viewport.width - size.width);
  const maxY = Math.max(0, viewport.height - size.height);
  return {
    x: Math.round(Math.max(0, Math.min(maxX, position.x))),
    y: Math.round(Math.max(0, Math.min(maxY, position.y))),
  };
}

export function readComposerSize(): ComposerSize | null {
  return readPoint(composerSizeKey, "width", "height", (value) => (
    value.width >= minimumComposerWidth && value.height >= minimumComposerHeight
  ));
}

export function readComposerPosition(): ComposerPosition | null {
  return readPoint(composerPositionKey, "x", "y", (value) => Number.isFinite(value.x) && Number.isFinite(value.y));
}

export function saveComposerSize(size: ComposerSize) {
  writeJson(composerSizeKey, size);
}

export function saveComposerPosition(position: ComposerPosition) {
  writeJson(composerPositionKey, position);
}

function readPoint<K extends string>(key: string, a: K, b: K, valid: (value: Record<K, number>) => boolean): Record<K, number> | null {
  try {
    const saved = JSON.parse(localStorage.getItem(key) ?? "null") as Partial<Record<K, number>> | null;
    if (saved && Number.isFinite(saved[a]) && Number.isFinite(saved[b])) {
      const value = { [a]: saved[a]!, [b]: saved[b]! } as Record<K, number>;
      if (valid(value)) return value;
    }
  } catch {
    // Keep the default layout when storage is unavailable or invalid.
  }
  return null;
}

function writeJson(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Retain the value for this session if persistence is unavailable.
  }
}
