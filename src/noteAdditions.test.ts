import { describe, expect, it } from "vitest";
import { appendBlock, composeAddition, formatQuote } from "./noteAdditions";
import type { NoteReference } from "./noteEntities";

const reference: NoteReference = { version: 1, kind: "conversation", id: "p1", account: "you@example.com" };
const id = "a".repeat(32);

/** The same cases as `src-tauri/src/note_additions.rs`, which writes the real note. */
describe("note additions, as the native add writes them", () => {
  it("quotes every line, keeps blank lines and inner markers, and defuses fences", () => {
    expect(formatQuote("one\ntwo")).toBe("> one\n> two\n");
    expect(formatQuote("one\r\n\r\ntwo\n\n")).toBe("> one\n>\n> two\n");
    expect(formatQuote("a > b\n> already\n>> deeper")).toBe("> a > b\n> > already\n> >> deeper\n");
    expect(formatQuote("```\ncode\n~~~")).toBe("> \\```\n> code\n> \\~~~\n");
  });

  it("writes the quote, its source line, and the files", () => {
    const block = composeAddition({ quote: "Hello\nthere", subject: "Q3 [plan]", reference, dateLabel: "Oct 30", linkPresent: false, files: [{ id, name: "a.png", image: true }, { id, name: "r.pdf", image: false }] })!;
    expect(block.startsWith("> Hello\n> there\n> — [Q3 \\[plan\\]](threestrands:conversation/")).toBe(true);
    expect(block).toContain(" · Oct 30\n\n");
    expect(block.endsWith(`![a.png](attachment:${id}) [r.pdf](attachment:${id})\n`)).toBe(true);
  });

  it("writes a bare link as a bullet, and nothing when the note has it", () => {
    expect(composeAddition({ quote: null, subject: "", reference, dateLabel: "", linkPresent: false })).toMatch(/^- \[\(No subject\)\]\(threestrands:conversation\/.*\)\n$/);
    expect(composeAddition({ quote: null, subject: "Plan", reference, dateLabel: "", linkPresent: true })).toBeNull();
    expect(composeAddition({ quote: " \n", subject: "Plan", reference, dateLabel: "", linkPresent: false })).toMatch(/^- \[Plan\]/);
  });

  it("appends after a blank line, except for a bullet", () => {
    expect(appendBlock("", "> q\n")).toBe("> q\n");
    expect(appendBlock("text", "> q\n")).toBe("text\n\n> q\n");
    expect(appendBlock("text\n", "> q\n")).toBe("text\n\n> q\n");
    expect(appendBlock("text\n\n", "> q\n")).toBe("text\n\n> q\n");
    expect(appendBlock("- a", "- [l](x)\n")).toBe("- a\n- [l](x)\n");
  });
});
