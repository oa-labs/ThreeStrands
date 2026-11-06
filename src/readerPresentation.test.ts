import { describe, expect, it, vi } from "vitest";
import { READER_PRESENTATION_KEY, READER_PRESENTATION_LIMIT, readReaderPresentation, saveReaderPresentation } from "./readerPresentation";

const presentation = { threadId: "thread", latestMessageId: "latest", activeMessageId: "older", expanded: [["older", true], ["latest", false]] as [string, boolean][] };

describe("reader presentation storage", () => {
  it("keeps the most recently used conversations, with one snapshot per thread", () => {
    for (let index = 0; index <= READER_PRESENTATION_LIMIT; index += 1) {
      saveReaderPresentation({ ...presentation, threadId: `thread-${index}` });
    }
    expect(readReaderPresentation("thread-0")).toBeUndefined();
    expect(readReaderPresentation("thread-1")).toBeDefined();
    saveReaderPresentation({ ...presentation, threadId: "thread-1", activeMessageId: "latest" });
    const entries = JSON.parse(localStorage.getItem(READER_PRESENTATION_KEY)!);
    expect(entries).toHaveLength(READER_PRESENTATION_LIMIT);
    expect(entries[0].activeMessageId).toBe("latest");
    expect(entries.filter((entry: { threadId: string }) => entry.threadId === "thread-1")).toHaveLength(1);
  });

  it("ignores malformed storage and invalid expansion values", () => {
    localStorage.setItem(READER_PRESENTATION_KEY, "{");
    expect(readReaderPresentation("thread")).toBeUndefined();
    localStorage.setItem(READER_PRESENTATION_KEY, JSON.stringify([null, { ...presentation, expanded: [["older", "false"]] }]));
    expect(readReaderPresentation("thread")).toBeUndefined();
    saveReaderPresentation(presentation);
    expect(readReaderPresentation("thread")).toEqual(presentation);
  });

  it("tolerates unavailable device storage", () => {
    const read = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("blocked"); });
    const write = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("blocked"); });
    try {
      expect(readReaderPresentation("thread")).toBeUndefined();
      expect(() => saveReaderPresentation(presentation)).not.toThrow();
    } finally {
      read.mockRestore();
      write.mockRestore();
    }
  });
});
