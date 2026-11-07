import { useState } from "react";

/** Compares plain data by its JSON form. */
export function jsonEqual<T>(left: T, right: T): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

/**
 * Returns the first value seen until one arrives that `equal` says differs.
 * Callers that build objects on each render can pass them straight in, and
 * effects that depend on the result run only when the value really changes.
 */
export function useStableValue<T>(value: T, equal: (left: T, right: T) => boolean = jsonEqual): T {
  const [stable, setStable] = useState(value);
  if (stable !== value && !equal(stable, value)) {
    // React's pattern for state derived from props: re-render now with the new value.
    setStable(value);
    return value;
  }
  return stable;
}
