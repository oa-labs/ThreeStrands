import { useEffect, useState } from "react";

/**
 * Below this window width Mail's right column becomes a drawer over the
 * reader, so the reader keeps room to read. Keep in step with the matching
 * media query in styles.css.
 */
export const NARROW_LAYOUT_QUERY = "(max-width: 1359px)";

function matches(query: string): boolean {
  return window.matchMedia?.(query).matches ?? false;
}

/** Whether a CSS media query matches, updated as the window changes. */
export function useMediaQuery(query: string): boolean {
  const [matched, setMatched] = useState(() => matches(query));
  useEffect(() => {
    const list = window.matchMedia?.(query);
    if (!list) return;
    const onChange = () => setMatched(list.matches);
    onChange();
    list.addEventListener("change", onChange);
    return () => list.removeEventListener("change", onChange);
  }, [query]);
  return matched;
}
