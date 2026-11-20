import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { ADD_TO_NOTE_MEETING_DAYS, MAX_ADDITION_FILES, MAX_ADDITION_QUOTE_CHARS, MAX_NOTE_ATTACHMENT_BYTES, fileFitsNote, quoteWithinLimit } from "./addToNotePolicy";

describe("Add to Note limits", () => {
  it("accepts a passage below and at the limit and refuses one above it, counting characters", () => {
    expect(quoteWithinLimit("x".repeat(MAX_ADDITION_QUOTE_CHARS - 1))).toBe(true);
    expect(quoteWithinLimit("x".repeat(MAX_ADDITION_QUOTE_CHARS))).toBe(true);
    expect(quoteWithinLimit("x".repeat(MAX_ADDITION_QUOTE_CHARS + 1))).toBe(false);
    expect(quoteWithinLimit("😀".repeat(MAX_ADDITION_QUOTE_CHARS))).toBe(true);
  });

  it("accepts a file below and at the note limit and refuses one above it", () => {
    expect(fileFitsNote(MAX_NOTE_ATTACHMENT_BYTES - 1)).toBe(true);
    expect(fileFitsNote(MAX_NOTE_ATTACHMENT_BYTES)).toBe(true);
    expect(fileFitsNote(MAX_NOTE_ATTACHMENT_BYTES + 1)).toBe(false);
  });

  it("names the same quote and file limits the native add enforces", () => {
    const native = readFileSync("src-tauri/src/note_additions.rs", "utf8");
    const value = (name: string) => Number(new RegExp(`pub const ${name}: usize = ([\\d_]+);`).exec(native)?.[1]?.replaceAll("_", ""));
    expect(value("MAX_ADDITION_QUOTE_CHARS")).toBe(MAX_ADDITION_QUOTE_CHARS);
    expect(value("MAX_ADDITION_FILES")).toBe(MAX_ADDITION_FILES);
    expect(ADD_TO_NOTE_MEETING_DAYS).toBeGreaterThan(0);
  });
});
