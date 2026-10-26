import type { OpenPromise } from "./domain";
import { parseAddress } from "./emailAddress";

const DISMISSED_KEY = "threestrands.today.dismissedPromises";

/** Most dismissed promises remembered; the oldest are forgotten first, long after their lookback has passed. */
export const MAX_DISMISSED_PROMISES = 200;

/**
 * Sent messages whose promise the user dismissed on this device. Dismissal is
 * device-local, like other view preferences; a promise dismissed on one
 * device can still show on another.
 */
export function readDismissedPromises(): string[] {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(DISMISSED_KEY) ?? "[]");
    return Array.isArray(stored) ? stored.filter((id): id is string => typeof id === "string") : [];
  } catch {
    return [];
  }
}

export function dismissPromise(messageId: string): string[] {
  const dismissed = [...readDismissedPromises().filter((id) => id !== messageId), messageId].slice(-MAX_DISMISSED_PROMISES);
  try {
    localStorage.setItem(DISMISSED_KEY, JSON.stringify(dismissed));
  } catch {
    // Dismissed for this session only.
  }
  return dismissed;
}

/** Who the promise was made to, by name where the To line gives one: "Jill", "Jill and Pete". */
export function promiseRecipients(promise: OpenPromise): string {
  const names = promise.recipients.map((recipient) => {
    const parsed = parseAddress(recipient);
    return parsed.name || parsed.email;
  });
  return new Intl.ListFormat(undefined, { type: "conjunction" }).format(names);
}
