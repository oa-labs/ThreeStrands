import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {  arrangePlanItems, insertPlanLines, parsePlan, planLine, removePlanLine, type PlanItemKind, type PlanMarker } from "./dailyPlan";
import { noteReferenceHref, type NoteReference } from "./noteEntities";

type Expected = { kind: PlanItemKind; text: string; marker?: PlanMarker; checked?: boolean };
const fixtures = JSON.parse(readFileSync("fixtures/daily-plan.json", "utf8")) as {
  references: Record<string, NoteReference>;
  parse: { name: string; body: string; expected: Expected[] | null; notToday?: Expected[] }[];
  insert: { name: string; body: string; lines: string[]; notToday?: boolean; expected: string }[];
  arrange: { name: string; body: string; order: ({ existing: number } | { new: string })[]; remove: number[]; expected: string }[];
};
const fill = (text: string) => Object.keys(fixtures.references).sort((a, b) => b.length - a.length)
  .reduce((out, name) => out.replaceAll(`{${name}}`, noteReferenceHref(fixtures.references[name])), text);
const normalize = (line: Expected) => ({ kind: line.kind, text: fill(line.text), marker: line.marker ?? null, checked: line.checked ?? false });

describe("daily plan", () => {
  it.each(fixtures.parse)("parses: $name", ({ body, expected, notToday }) => {
    const filled = fill(body);
    const section = parsePlan("daily:2026-11-16", filled);
    const lines = (items: NonNullable<typeof section>["items"]) => items.map((item) => ({ kind: item.kind, text: filled.slice(item.start, item.end), marker: item.marker, checked: item.checked }));
    expect(section ? lines(section.items) : null).toEqual(expected ? expected.map(normalize) : null);
    expect(section ? lines(section.notToday) : []).toEqual((notToday ?? []).map(normalize));
  });

  it.each(fixtures.insert)("inserts: $name", ({ body, lines, notToday, expected }) => {
    let actual: string;
    try { actual = insertPlanLines("daily:2026-11-16", fill(body), lines.map(fill), notToday ?? false); } catch (error) { actual = `ERROR: ${(error as Error).message}`; }
    expect(actual).toBe(fill(expected));
  });

  it.each(fixtures.arrange)("arranges: $name", ({ body, order, remove, expected }) => {
    const filled = fill(body);
    let actual: string;
    try { actual = arrangePlanItems("daily:2026-11-16", filled, parsePlan("daily:2026-11-16", filled), order.map((entry) => "new" in entry ? { new: fill(entry.new) } : entry), remove); } catch { actual = "ERROR"; }
    expect(actual).toBe(fill(expected));
  });

  it("reads plans whose notes still hold a Shutdown paragraph from earlier versions", () => {
    // Earlier versions appended notes on the day under `### Shutdown`; they stay prose, never plan items.
    const body = "## Plan\n\n- a\n\n### Shutdown\n\nOne.\n\n\\- call Sam\n\n1\\. later\n\n## Meetings\n\n- 9am Sync\n";
    const section = parsePlan("n", body)!;
    expect(section.items.map((item) => item.label)).toEqual(["a"]);
    expect(section.notToday).toHaveLength(0);
  });

  it("writes link and task lines with markers, and removes one item", () => {
    const task: NoteReference = { version: 1, kind: "task", id: "t1" };
    expect(planLine({ kind: "link", reference: task, label: "Send *Q3* deck" }, { kind: "carried", date: "2026-11-15" }))
      .toBe(`- [Send \\*Q3\\* deck](${noteReferenceHref(task)}) <!-- threestrands-plan:carried 2026-11-15 -->`);
    expect(planLine({ kind: "newTask", title: " Draft   memo " })).toBe("- ( ) Draft memo");
    expect(() => planLine({ kind: "newTask", title: " " })).toThrow();
    const body = "## Plan\n\n- a\n  - nested\n- b\n";
    const section = parsePlan("n", body)!;
    expect(removePlanLine(body, section.items[0].start, section.items[0].end)).toBe("## Plan\n\n- b\n");
    expect(removePlanLine(body, section.items[1].start, section.items[1].end)).toBe("## Plan\n\n- a\n  - nested\n");
  });
});
