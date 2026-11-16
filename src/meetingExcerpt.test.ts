import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { boundedText, occurrenceBlocks } from "./meetingExcerpt";
import { noteReferenceHref, type NoteReference } from "./noteEntities";

const fixtures = JSON.parse(readFileSync("fixtures/meeting-excerpts.json", "utf8")) as {
  events: Record<string, NoteReference>;
  target: string;
  cases: { name: string; body: string; blocks: { kind: string; text: string }[] }[];
};

// Longest names first, so `{A}` doesn't match inside `{A_OTHER_CALENDAR}`.
const fill = (text: string) => Object.keys(fixtures.events).sort((a, b) => b.length - a.length)
  .reduce((out, name) => out.replaceAll(`{${name}}`, noteReferenceHref(fixtures.events[name])), text);

describe("meeting excerpts", () => {
  it.each(fixtures.cases)("$name", ({ body, blocks }) => {
    const filled = fill(body);
    const actual = occurrenceBlocks(filled, fixtures.events[fixtures.target]).map((block) => ({ kind: block.kind, text: filled.slice(block.start, block.end) }));
    expect(actual).toEqual(blocks.map((block) => ({ kind: block.kind, text: fill(block.text) })));
  });

  it("does not treat a series link as this occurrence", () => {
    const occurrence: NoteReference = { version: 1, kind: "event", id: "evt-a", account: "you@example.com", calendar: "work", series: "series" };
    const body = `- [Series](${noteReferenceHref({ ...occurrence, id: "series", series: undefined })})\n- [This one](${noteReferenceHref(occurrence)})\n`;
    const blocks = occurrenceBlocks(body, occurrence);
    expect(blocks).toHaveLength(1);
    expect(body.slice(blocks[0].start, blocks[0].end)).toMatch(/^- \[This one\]/);
  });

  it("cuts text below, at, and above the byte limit on character boundaries", () => {
    expect(boundedText("abc", 4)).toEqual({ text: "abc", truncated: false });
    expect(boundedText("abcd", 4)).toEqual({ text: "abcd", truncated: false });
    expect(boundedText("abcde", 4)).toEqual({ text: "abcd", truncated: true });
    expect(boundedText("aé", 2)).toEqual({ text: "a", truncated: true });
  });
});
