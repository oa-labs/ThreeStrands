import { useEffect, useState } from "react";
import { mailClient } from "./data/client";
import { logBackgroundFailure } from "./errors";

/** Shorter text matches too much of a calendar to be worth a search; the native search agrees. */
export const MIN_LOCATION_QUERY_CHARS = 3;
/** Wait for a pause in typing so each keystroke does not search every calendar. */
export const LOCATION_SEARCH_DELAY_MS = 250;

/**
 * Places from the user's calendars that contain `text`, searched after a pause
 * in typing. Only the latest text's answer is kept, so a slow search for an
 * earlier prefix never replaces newer suggestions. Disabled or short text
 * clears them.
 */
export function useLocationSuggestions(text: string, enabled: boolean): string[] {
  const [suggestions, setSuggestions] = useState<string[]>([]);
  const query = text.trim();
  const searchable = enabled && [...query].length >= MIN_LOCATION_QUERY_CHARS;
  useEffect(() => {
    if (!searchable) {
      setSuggestions([]);
      return;
    }
    let current = true;
    const timer = setTimeout(() => {
      void mailClient.searchEventLocations(query).then((found) => {
        if (current) setSuggestions(found);
      }).catch(logBackgroundFailure("Event location lookup"));
    }, LOCATION_SEARCH_DELAY_MS);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [query, searchable]);
  return searchable ? suggestions : [];
}
