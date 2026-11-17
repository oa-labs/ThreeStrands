import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { docToMarkdown, markdownToDoc } from "@meowdown/core";
import { parsePlan } from "./dailyPlan";
import { contactNoteReferences, fromEditorMarkdown, toEditorMarkdown, extractNoteEntityLinks, noteItemRanges, noteReferenceHref, noteReferenceKey, noteReferenceSeriesKey, normalizeContactAddress, parseNoteReference, MAX_NOTE_REFERENCE_BYTES } from "./noteEntities";

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
  it("names an event occurrence with or without its series, and keys the series like a link to it", () => {
    const occurrence = { version: 1 as const, kind: "event" as const, id: "standup_1112", account: "you@example.com", calendar: "work", series: "standup" };
    expect(parseNoteReference(noteReferenceHref(occurrence))).toEqual(occurrence);
    expect(noteReferenceKey(occurrence)).toBe(noteReferenceKey({ ...occurrence, series: undefined }));
    expect(noteReferenceSeriesKey(occurrence)).toBe(noteReferenceKey({ ...occurrence, id: "standup", series: undefined }));
    expect(noteReferenceSeriesKey({ ...occurrence, series: undefined })).toBeNull();
    const encoded = (value: object) => `threestrands:${(value as { kind: string }).kind}/${encodeURIComponent(JSON.stringify(value))}`;
    expect(parseNoteReference(encoded({ ...occurrence, series: "standup_1112" }))).toBeNull();
    expect(parseNoteReference(encoded({ version: 1, kind: "task", id: "t", series: "s" }))).toBeNull();
    expect(parseNoteReference(encoded({ ...occurrence, series: null }))).toBeNull();
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

  describe("trailing metadata comments in the editor view", () => {
    const T = "<!-- threestrands-task:4ec77948-4813-5e64-93f9-c660ae48947f -->";
    const carried = "<!-- threestrands-plan:carried 2026-11-16 -->", deferred = "<!-- threestrands-plan:deferred 2026-11-20 -->", dropped = "<!-- threestrands-plan:dropped -->";
    const followThrough = "<!-- threestrands-follow-through -->";
    /** What the editor library writes for `view`: it separates every item of a list holding a block comment. */
    const throughEditor = (view: string) => docToMarkdown(markdownToDoc(view));
    const hidden = (view: string, comment: string) => expect(view).toContain(`\n\n  ${comment}`);
    const plan = `# Monday\n\n## Plan\n\n- [Send deck](threestrands:task/a) ${carried}\n  - with the appendix ${carried}\n- [Memo](https://example.com/m)\n- ( ) Draft ${T} ${carried}\n- ( ) Write ${carried} ${T}\n- (x) Book room\n- Think about Q4 ${carried}\n\n### Not today\n\n- [Old](threestrands:task/b) ${deferred}\n- Survey ${dropped}\n\n### Shutdown\n\n- Done early.\n\n## Meetings\n\n- 9am Sync\n`;

    it.each([
      ["a plain item", `- Free ${carried}\n`, "  "],
      ["a link item", `- [Report](https://example.com/x) ${deferred}\n`, "  "],
      ["a dropped item", `- Gone ${dropped}\n`, "  "],
      ["a task id then a plan marker", `- ( ) Proposal ${T} ${carried}\n`, "  "],
      ["an ordered item, indented to its content", `1. First ${carried}\n2. Second\n`, "   "],
      ["a nested item", `- Outer\n  - Inner ${carried}\n`, "    "],
      ["a follow-through section line", `- Follow-through 2026-11-16: [Sync](https://example.com/s) ${followThrough}\n  - Send notes\n`, "  "],
    ])("moves the markers of %s onto an indented block and back", (_name, source, indent) => {
      const view = toEditorMarkdown(source);
      expect(view).not.toBe(source);
      expect(view).toContain(`\n\n${indent}<!--`);
      expect(fromEditorMarkdown(view, source)).toBe(source);
      expect(fromEditorMarkdown(throughEditor(view), source)).toBe(source);
    });
    it("hides a whole plan section and restores it byte for byte, even from a loose editor list", () => {
      const view = toEditorMarkdown(plan);
      expect(view.split("\n").filter((line) => /^\s*(?:[-+*]|\d+[.)])\s/.test(line) && line.includes("<!--"))).toEqual([]);
      hidden(view, `${T} ${carried}`);
      hidden(view, `${carried} ${T}`);
      expect(fromEditorMarkdown(view, plan)).toBe(plan);
      expect(fromEditorMarkdown(throughEditor(view), plan)).toBe(plan);
    });
    it("keeps a marker on its item when the item's text is edited", () => {
      const view = toEditorMarkdown(plan).replace("Think about Q4", "Think about Q5");
      expect(fromEditorMarkdown(throughEditor(view), plan)).toBe(plan.replace("Think about Q4", "Think about Q5"));
    });
    it("leaves anything but a validated, adjacent marker as authored content", () => {
      for (const source of [
        `- Free <!-- threestrands-plan:carried tomorrow -->\n`,
        `- Free <!-- threestrands-plan:carried 2026-13-45 -->\n`,
        `- Free <!-- threestrands-plan:dropped now -->\n`,
        `- Free <!-- threestrands-plan:other -->\n`,
        `- Free <!-- threestrands-task:not-a-uuid -->\n`,
        `- Free  ${carried}\n`,
        `- Free ${carried} trailing\n`,
        `- <!-- threestrands-plan:dropped -->\n`,
        `> - Quoted ${carried}\n`,
        "```\n- In code " + carried + "\n```\n",
        `    - indented code ${carried}\n`,
        `<!--\n- in comment ${carried}\n-->\n`,
        `Paragraph ${carried}\n`,
      ]) {
        const view = toEditorMarkdown(source);
        expect(view, source).not.toMatch(/\n\n\s*<!-- threestrands-plan:(?:carried 20|dropped -->)/);
        expect(fromEditorMarkdown(view, source), source).toBe(source);
      }
      const stray = `- Item\n\n  ${carried}\n\nText ${T}\n`;
      expect(fromEditorMarkdown(stray, stray)).toBe(`- Item ${carried}\n\nText ${T}\n`.replace(/^- Item ([^\n]*)\n/, `- Item $1\n`));
      expect(fromEditorMarkdown(`- Item\n\n  <!-- note -->\n`, "")).toBe(`- Item\n\n  <!-- note -->\n`);
    });
    it("a note saved by the previous release's editor, which kept the markers inline, loads and saves unchanged", () => {
      expect(throughEditor(plan)).toBe(plan);
      const view = toEditorMarkdown(plan);
      expect(fromEditorMarkdown(view, plan)).toBe(plan);
      // Opened without a saved copy to compare against, the markers still fold back.
      expect(fromEditorMarkdown(view)).toContain(`- Think about Q4 ${carried}\n`);
    });
    it("a note saved through the new view reads the same plan as before", () => {
      const saved = fromEditorMarkdown(throughEditor(toEditorMarkdown(plan)), plan);
      expect(parsePlan("daily:2026-11-16", saved)).toEqual(parsePlan("daily:2026-11-16", plan));
      expect(parsePlan("daily:2026-11-16", saved)!.items.filter((i) => i.marker).length).toBe(3);
      expect(parsePlan("daily:2026-11-16", saved)!.notToday.map((i) => i.marker)).toEqual([{ kind: "deferred", date: "2026-11-20" }, { kind: "dropped" }]);
    });
  });
});
