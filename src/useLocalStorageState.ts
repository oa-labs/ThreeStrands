import { useCallback, useEffect, useState, type Dispatch, type SetStateAction } from "react";

/** Parsing and storage access both fail back to a usable session preference. */
export function readLocalStorage<T>(key: string, parse: (raw: string | null) => T, fallback: T): T {
  try { return parse(localStorage.getItem(key)); } catch { return fallback; }
}

export function writeLocalStorage(key: string, value: string): void {
  try { localStorage.setItem(key, value); } catch { /* The session preference still applies. */ }
}

/** Keep existing storage formats via the caller's reader and serializer. */
export function useLocalStorageState<T>(
  key: string,
  read: () => T,
  serialize: (value: T) => string = String,
  delayMs = 0,
): [T, Dispatch<SetStateAction<T>>] {
  const [state, setState] = useState(() => ({ key, value: read() }));
  // A workspace may reuse the component with a different preference key.
  if (state.key !== key) setState({ key, value: read() });
  const value = state.value;
  useEffect(() => {
    if (state.key !== key) return;
    const save = () => {
      try { writeLocalStorage(key, serialize(value)); } catch { /* Serialization may also fail. */ }
    };
    if (!delayMs) { save(); return; }
    const timer = window.setTimeout(save, delayMs);
    return () => window.clearTimeout(timer);
  }, [key, state.key, value, serialize, delayMs]);
  const setValue: Dispatch<SetStateAction<T>> = useCallback((next) => setState((current) => {
    const value = typeof next === "function" ? (next as (value: T) => T)(current.value) : next;
    return Object.is(value, current.value) ? current : { key: current.key, value };
  }), []);
  return [value, setValue];
}
