import { invoke, type InvokeArgs, type InvokeOptions } from "@tauri-apps/api/core";

const BOUNDED_READ_TIMEOUT_MS = 30_000;

export type InvokePolicy =
  | { readonly timeout: "none" }
  | { readonly timeout: "bounded-read"; readonly timeoutMs: number };

/**
 * Commands which involve a person, network I/O, or a state change must remain
 * attached to their native invocation. Tauri cannot cancel native work when a
 * frontend Promise is rejected, so a synthetic timeout would make completion
 * ambiguous and could encourage a duplicate retry.
 */
export const WAIT_FOR_NATIVE_COMPLETION: InvokePolicy = { timeout: "none" };

/**
 * A fail-fast policy reserved for local, read-only commands. Retrying one of
 * these commands cannot duplicate a side effect if native work finishes late.
 */
export const BOUNDED_LOCAL_READ: InvokePolicy = {
  timeout: "bounded-read",
  timeoutMs: BOUNDED_READ_TIMEOUT_MS,
};

export class InvokeTimeoutError extends Error {
  constructor(command: string) {
    super(`"${command}" timed out`);
    this.name = "InvokeTimeoutError";
  }
}

/**
 * Invokes a native command under an explicit lifecycle policy. Requiring the
 * policy at each call site prevents new long-running or state-changing work
 * from accidentally inheriting a universal frontend timeout.
 */
export function invokeWithPolicy<T>(
  command: string,
  args: InvokeArgs | undefined,
  policy: InvokePolicy,
  options?: InvokeOptions,
): Promise<T> {
  // Options only when given, so a call reads the same as before they existed.
  const call = () => options ? invoke<T>(command, args, options) : invoke<T>(command, args);
  if (policy.timeout === "none") {
    return call();
  }

  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new InvokeTimeoutError(command)),
      policy.timeoutMs,
    );
    call().then(
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
