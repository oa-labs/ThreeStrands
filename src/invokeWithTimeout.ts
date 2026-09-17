import { invoke, type InvokeArgs } from "@tauri-apps/api/core";

/** Generous enough that it never fires under normal latency; only guards against a truly hung backend call. */
const DEFAULT_TIMEOUT_MS = 30_000;

export class InvokeTimeoutError extends Error {
  constructor(command: string) {
    super(`"${command}" timed out`);
    this.name = "InvokeTimeoutError";
  }
}

/**
 * Wraps `@tauri-apps/api/core`'s `invoke` with a timeout so a hung backend
 * command rejects instead of leaving a caller (e.g. the composer's `busy`
 * guard) stuck forever with no way to recover short of a force-quit.
 */
export function invokeWithTimeout<T>(
  command: string,
  args?: InvokeArgs,
  timeoutMs = DEFAULT_TIMEOUT_MS,
): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new InvokeTimeoutError(command)), timeoutMs);
    invoke<T>(command, args).then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}
