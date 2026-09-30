/** Matches `SyncService` polling: 15s after activity, backing off while idle. */
export const FOREGROUND_IDLE_MS = 15_000;
export const FOREGROUND_DEBOUNCE_MS = 250;

export type ForegroundClock = {
  now(): number;
  setTimeout(callback: () => void, ms: number): number;
  clearTimeout(id: number): void;
};

const browserClock: ForegroundClock = {
  now: () => Date.now(),
  setTimeout: (callback, ms) => window.setTimeout(callback, ms),
  clearTimeout: (id) => window.clearTimeout(id),
};

/**
 * Debounces focus/visibility flicker and skips a catch-up sync until the app
 * has been in the background (or unfocused) for at least the polling floor.
 */
export function createForegroundRefreshController(
  refresh: () => void,
  clock: ForegroundClock = browserClock,
) {
  let backgroundedAt: number | null = null;
  let debounceId = 0;

  const cancelScheduled = () => {
    if (!debounceId) return;
    clock.clearTimeout(debounceId);
    debounceId = 0;
  };

  return {
    onBackground() {
      cancelScheduled();
      if (backgroundedAt === null) backgroundedAt = clock.now();
    },
    onForeground() {
      cancelScheduled();
      const awayStarted = backgroundedAt;
      const foregroundAt = clock.now();
      debounceId = clock.setTimeout(() => {
        debounceId = 0;
        // The foreground has settled, so the absence is over whether or not
        // it was long enough; the next one is measured from scratch.
        backgroundedAt = null;
        if (awayStarted === null) return;
        if (foregroundAt - awayStarted < FOREGROUND_IDLE_MS) return;
        refresh();
      }, FOREGROUND_DEBOUNCE_MS);
    },
    dispose: cancelScheduled,
  };
}
