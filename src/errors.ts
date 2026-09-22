/**
 * Human-readable text for a rejection reason. Tauri commands reject with plain
 * strings while browser APIs reject with `Error`s, so both are normalized to
 * the message alone (no "Error: " prefix).
 */
export function errorMessage(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}

/**
 * Rejection handler for best-effort background work — status polls, counts,
 * reconciliation, autosave retries — where the UI has nothing to show and the
 * next attempt will try again. The failure is logged instead of vanishing so
 * it stays diagnosable from the developer console.
 */
export function logBackgroundFailure(task: string): (reason: unknown) => void {
  return (reason) => {
    console.warn(`${task} failed:`, reason);
  };
}
