import type { ReaderAnchor } from "./returnNavigation";

/**
 * Where the user stopped reading recently opened conversations, so reopening
 * a long newsletter from the list lands back on their place. Kept per device,
 * outside the `threestrands.settings.` namespace and the settings-transfer
 * allowlist: it is a reading convenience, not a preference. Storage can be
 * unavailable (private windows, blocked site data), so every access tolerates
 * failure.
 */

export const READING_POSITIONS_KEY = "threestrands.readingPositions";
/** Oldest positions drop off past this many conversations. */
export const READING_POSITION_LIMIT = 100;
/** A position older than this is forgotten: reopening an old email is usually looking something up, not resuming. */
export const READING_POSITION_MAX_AGE_MS = 2 * 24 * 60 * 60 * 1000;

/** A conversation's place, and the latest message it had then: a newer message means it should open on that instead. */
export type ReadingPosition = { threadId: string; latestMessageId: string; anchor: ReaderAnchor; savedAt: number };

/** Where the reader is in an open conversation, as the reader tracks it before saving. */
export type ReadingPlace = {
  threadId: string;
  latestMessageId: string;
  anchor: ReaderAnchor;
  viewportHeight: number;
  atBottom: boolean;
};

/**
 * Only a place worth coming back to: scrolled at least a screen into the
 * latest message, or up into an earlier one, and not finished.
 */
export function worthRemembering(place: ReadingPlace): boolean {
  if (place.atBottom) return false;
  return place.anchor.messageId !== place.latestMessageId || place.anchor.offset >= place.viewportHeight;
}

function isPosition(value: unknown): value is ReadingPosition {
  if (!value || typeof value !== "object") return false;
  const entry = value as Record<string, unknown>;
  const anchor = entry.anchor as Record<string, unknown> | null | undefined;
  return typeof entry.threadId === "string"
    && typeof entry.latestMessageId === "string"
    && typeof entry.savedAt === "number"
    && Boolean(anchor) && typeof anchor?.messageId === "string" && typeof anchor.offset === "number" && Number.isFinite(anchor.offset);
}

export function readReadingPositions(): ReadingPosition[] {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(READING_POSITIONS_KEY) ?? "[]");
    return Array.isArray(parsed) ? parsed.filter(isPosition).slice(0, READING_POSITION_LIMIT) : [];
  } catch {
    return [];
  }
}

/**
 * Puts the conversation's place first (or removes it when `place` is not worth
 * remembering), dropping expired and overflow entries.
 */
export function updateReadingPositions(
  positions: readonly ReadingPosition[],
  place: ReadingPlace,
  now: number,
): ReadingPosition[] {
  const others = positions.filter((entry) => entry.threadId !== place.threadId && now - entry.savedAt < READING_POSITION_MAX_AGE_MS);
  const next = worthRemembering(place)
    ? [{ threadId: place.threadId, latestMessageId: place.latestMessageId, anchor: place.anchor, savedAt: now }, ...others]
    : others;
  return next.slice(0, READING_POSITION_LIMIT);
}

export function saveReadingPlace(place: ReadingPlace, now = Date.now()) {
  try {
    localStorage.setItem(READING_POSITIONS_KEY, JSON.stringify(updateReadingPositions(readReadingPositions(), place, now)));
  } catch {
    // Reopening simply starts at the latest message.
  }
}

/** The place to reopen a conversation at, unless it expired or a newer message arrived since. */
export function readingPositionFor(
  positions: readonly ReadingPosition[],
  threadId: string,
  latestMessageId: string,
  now: number,
): ReaderAnchor | null {
  const entry = positions.find((candidate) => candidate.threadId === threadId);
  if (!entry || entry.latestMessageId !== latestMessageId || now - entry.savedAt >= READING_POSITION_MAX_AGE_MS) return null;
  return entry.anchor;
}
