/** Device-local conversation presentation; deliberately excluded from settings transfer. */
export const READER_PRESENTATION_KEY = "threestrands.readerPresentation";
export const READER_PRESENTATION_LIMIT = 100;
export type ReaderPresentation = {
  threadId: string;
  latestMessageId: string;
  activeMessageId: string | null;
  expanded: [string, boolean][];
};

function readPresentations(): ReaderPresentation[] {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(READER_PRESENTATION_KEY) ?? "[]");
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((entry): entry is ReaderPresentation => entry && typeof entry === "object"
      && typeof entry.threadId === "string" && typeof entry.latestMessageId === "string"
      && (entry.activeMessageId === null || typeof entry.activeMessageId === "string")
      && Array.isArray(entry.expanded) && entry.expanded.every((pair: unknown) => Array.isArray(pair)
        && pair.length === 2 && typeof pair[0] === "string" && typeof pair[1] === "boolean"))
      .slice(0, READER_PRESENTATION_LIMIT);
  } catch {
    return [];
  }
}

export function readReaderPresentation(threadId: string): ReaderPresentation | undefined {
  return readPresentations().find((entry) => entry.threadId === threadId);
}

export function saveReaderPresentation(presentation: ReaderPresentation) {
  try {
    localStorage.setItem(READER_PRESENTATION_KEY, JSON.stringify([
      presentation, ...readPresentations().filter((entry) => entry.threadId !== presentation.threadId),
    ].slice(0, READER_PRESENTATION_LIMIT)));
  } catch {
    // The reader still works when device storage is unavailable.
  }
}
