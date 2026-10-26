import { afterEach, describe, expect, it, vi } from "vitest";
import {
  READING_POSITION_LIMIT,
  READING_POSITION_MAX_AGE_MS,
  READING_POSITIONS_KEY,
  readingPositionFor,
  readReadingPositions,
  saveReadingPlace,
  updateReadingPositions,
  worthRemembering,
  type ReadingPlace,
} from "./readingPositions";

const place = (overrides: Partial<ReadingPlace> = {}): ReadingPlace => ({
  threadId: "thread-1",
  latestMessageId: "latest",
  anchor: { messageId: "latest", offset: 1800 },
  viewportHeight: 600,
  atBottom: false,
  ...overrides,
});

describe("reading positions", () => {
  afterEach(() => {
    localStorage.removeItem(READING_POSITIONS_KEY);
    vi.restoreAllMocks();
  });

  it("remembers a place at least a screen into the latest message, or in an earlier one, until it is finished", () => {
    expect(worthRemembering(place())).toBe(true);
    expect(worthRemembering(place({ anchor: { messageId: "latest", offset: 600 } }))).toBe(true);
    expect(worthRemembering(place({ anchor: { messageId: "latest", offset: 599 } }))).toBe(false);
    expect(worthRemembering(place({ anchor: { messageId: "older", offset: 0 } }))).toBe(true);
    expect(worthRemembering(place({ atBottom: true }))).toBe(false);
  });

  it("keeps the newest place first, one per conversation, and forgets a place no longer worth keeping", () => {
    let positions = updateReadingPositions([], place(), 1000);
    positions = updateReadingPositions(positions, place({ threadId: "thread-2" }), 2000);
    positions = updateReadingPositions(positions, place({ anchor: { messageId: "latest", offset: 2400 } }), 3000);
    expect(positions.map((entry) => [entry.threadId, entry.anchor.offset, entry.savedAt])).toEqual([
      ["thread-1", 2400, 3000],
      ["thread-2", 1800, 2000],
    ]);

    positions = updateReadingPositions(positions, place({ atBottom: true }), 4000);
    expect(positions.map((entry) => entry.threadId)).toEqual(["thread-2"]);
  });

  it("holds at most the limit, dropping the oldest", () => {
    let positions = updateReadingPositions([], place({ threadId: "first" }), 0);
    for (let index = 1; index <= READING_POSITION_LIMIT; index += 1) {
      positions = updateReadingPositions(positions, place({ threadId: `thread-${index}` }), index);
    }
    expect(positions).toHaveLength(READING_POSITION_LIMIT);
    expect(positions[0].threadId).toBe(`thread-${READING_POSITION_LIMIT}`);
    expect(positions.some((entry) => entry.threadId === "first")).toBe(false);
  });

  it("drops expired places when saving", () => {
    const positions = updateReadingPositions([], place({ threadId: "old" }), 0);
    expect(updateReadingPositions(positions, place(), READING_POSITION_MAX_AGE_MS - 1).map((entry) => entry.threadId)).toEqual(["thread-1", "old"]);
    expect(updateReadingPositions(positions, place(), READING_POSITION_MAX_AGE_MS).map((entry) => entry.threadId)).toEqual(["thread-1"]);
  });

  it("resumes only before the place expires and while the same message is still the latest", () => {
    const positions = updateReadingPositions([], place(), 1000);
    const anchor = { messageId: "latest", offset: 1800 };
    expect(readingPositionFor(positions, "thread-1", "latest", 1000 + READING_POSITION_MAX_AGE_MS - 1)).toEqual(anchor);
    expect(readingPositionFor(positions, "thread-1", "latest", 1000 + READING_POSITION_MAX_AGE_MS)).toBeNull();
    expect(readingPositionFor(positions, "thread-1", "newer", 1000)).toBeNull();
    expect(readingPositionFor(positions, "thread-2", "latest", 1000)).toBeNull();
  });

  it("stores places on this device and treats corrupt or foreign data as empty", () => {
    saveReadingPlace(place(), 1000);
    expect(readReadingPositions()).toEqual([{ threadId: "thread-1", latestMessageId: "latest", anchor: { messageId: "latest", offset: 1800 }, savedAt: 1000 }]);

    localStorage.setItem(READING_POSITIONS_KEY, "{not json");
    expect(readReadingPositions()).toEqual([]);
    localStorage.setItem(READING_POSITIONS_KEY, JSON.stringify([
      null,
      { threadId: "a", latestMessageId: "m", savedAt: 1 },
      { threadId: "b", latestMessageId: "m", savedAt: 1, anchor: { messageId: "m", offset: "12" } },
      { threadId: "c", latestMessageId: "m", savedAt: 1, anchor: { messageId: "m", offset: 12 } },
    ]));
    expect(readReadingPositions().map((entry) => entry.threadId)).toEqual(["c"]);
  });

  it("keeps working when storage is unavailable", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("blocked"); });
    expect(() => saveReadingPlace(place())).not.toThrow();
  });
});
