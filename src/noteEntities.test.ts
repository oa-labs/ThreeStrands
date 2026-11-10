import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { docToMarkdown, markdownToDoc } from "@meowdown/core";
import { contactNoteReferences, fromEditorMarkdown, toEditorMarkdown, extractNoteEntityLinks, noteItemRanges, noteReferenceHref, noteReferenceKey, normalizeContactAddress, parseNoteReference, MAX_NOTE_REFERENCE_BYTES } from "./noteEntities";

const fixtures = JSON.parse(readFileSync("fixtures/note-entities.json", "utf8")) as { name: string; body: string; kinds: boolean[]; checked: boolean[] }[];
const ID = "4ec77948-4813-5e64-93f9-c660ae48947f";
describe("note entity Markdown contract", () => {
  it.each(fixtures)("shares the native parser's $name grammar", ({ body, kinds, checked }) => {
    const items = noteItemRanges(body);
    expect(items.map((i) => i.task)).toEqual(kinds);
    expect(items.map((i) => i.checked)).toEqual(checked);
  });
  it("adapts round tasks while retaining square plus lists and ordered prefixes", () => {
    const body = `+ [ ] Shopping\n1. ( ) Proposal <!-- threestrands-task:${ID} -->\n`;
    const editor = toEditorMarkdown(body);
    expect(editor).toContain("- [ ] Shopping");
    expect(editor).toContain("+ [ ] Proposal");
    expect(fromEditorMarkdown(editor, body)).toBe(body);
    expect(fromEditorMarkdown(editor.replace("+ [ ]", "+ [x]"), body)).toContain("1. (x) Proposal");
  });
  it("keeps code and prose parentheses unchanged through the editor adapter", () => {
    const source = "```md\n- ( ) code\n```\nProse ( ) unchanged\n";
    expect(toEditorMarkdown(source)).toBe(source);
    expect(fromEditorMarkdown(source)).toBe(source);
  });
  it("the previously shipped editor preserves parenthesized tasks and ids on unrelated edits", () => {
    const body = `- ( ) Send proposal <!-- threestrands-task:${ID} -->\n\nOther paragraph\n`;
    const oldEditorOutput = docToMarkdown(markdownToDoc(body));
    expect(noteItemRanges(oldEditorOutput)[0]).toMatchObject({ task: true, checked: false, id: ID });
    expect(oldEditorOutput.replace("Other paragraph", "Edited elsewhere")).toContain(`threestrands-task:${ID}`);
  });
  it("shares byte-identical canonical reference encoding with native", () => {
    const fixtures = JSON.parse(readFileSync("fixtures/note-references.json", "utf8")) as { reference: Parameters<typeof noteReferenceHref>[0]; href: string }[];
    for (const fixture of fixtures) { expect(noteReferenceHref(fixture.reference)).toBe(fixture.href); expect(parseNoteReference(fixture.href)).toEqual(fixture.reference); }
  });
  it("names a contact by its normalized address, with the contact id only as a hint", () => {
    const pete = { version: 1 as const, kind: "contact" as const, id: "pete@example.com" };
    expect(parseNoteReference(noteReferenceHref({ ...pete, contact: "c1" }))).toEqual({ ...pete, contact: "c1" });
    expect(noteReferenceKey({ ...pete, contact: "c1" })).toBe(noteReferenceKey(pete));
    for (const id of ["Pete@example.com", " pete@example.com", "pete example@example.com", "pete", "@example.com", "pete@", "a@b@example.com"]) expect(() => noteReferenceHref({ ...pete, id }), id).toThrow();
    const encoded = (value: object) => `threestrands:contact/${encodeURIComponent(JSON.stringify(value))}`;
    for (const extra of [{ account: "you@example.com" }, { note: "n" }, { contact: "" }, { contact: null }, { name: "Pete" }]) expect(parseNoteReference(encoded({ ...pete, ...extra }))).toBeNull();
    expect(parseNoteReference(encoded({ version: 1, kind: "task", id: "t", contact: "c1" }))).toBeNull();
    expect(contactNoteReferences([" Pete@Example.com", "pete@example.com", "not an address", "frank@example.com"]).map((r) => r.id)).toEqual(["pete@example.com", "frank@example.com"]);
    expect(normalizeContactAddress(" Pete@Example.COM ")).toBe("pete@example.com");
  });
  it("the previously shipped editor keeps contact links byte for byte", () => {
    const href = noteReferenceHref({ version: 1, kind: "contact", id: "pete.b+ops@example.com", contact: "c-1" });
    const body = `- 9:30am met with [Pete B](${href}) for planning\n\nOther paragraph\n`;
    const oldEditorOutput = docToMarkdown(markdownToDoc(body));
    expect(oldEditorOutput).toContain(`[Pete B](${href})`);
    expect(extractNoteEntityLinks(oldEditorOutput).links.map((link) => link.href)).toEqual([href]);
  });
  it("indexes quoted links but keeps quoted code inert", () => {
    const href = noteReferenceHref({ version: 1, kind: "task", id: "task" });
    expect(extractNoteEntityLinks(`> [Link](${href})`).links).toHaveLength(1);
    expect(extractNoteEntityLinks(`> ~~~md\n> [Code](${href})\n> ~~~`).links).toHaveLength(0);
    expect(extractNoteEntityLinks(`>     [Code](${href})`).links).toHaveLength(0);
  });
  it("strictly validates versioned references and escapes all destination delimiters", () => {
    const ref = { version: 1 as const, kind: "conversation" as const, id: "邮件/(a)", account: "you@example.com" };
    expect(parseNoteReference(noteReferenceHref(ref))).toEqual(ref);
    for (const invalid of ["javascript:alert(1)", "file:///etc/passwd", "threestrands:task/%xx", "threestrands:task/{}", noteReferenceHref(ref).replace("conversation/", "event/")]) expect(parseNoteReference(invalid)).toBeNull();
    const unknown = `threestrands:task/${encodeURIComponent(JSON.stringify({ version: 1, kind: "task", id: "a", command: "run" }))}`;
    expect(parseNoteReference(unknown)).toBeNull();
    expect(() => noteReferenceHref({ version: 1, kind: "task", id: "x".repeat(MAX_NOTE_REFERENCE_BYTES) })).toThrow();
  });
});
