import { noteInlineText } from "./notePreview";
import { expect, it } from "vitest";
import { extractNoteEntityLinks, noteReferenceHref } from "./noteEntities";
it("keeps wiki links and nested bracket text separate from entity-link labels", () => {
  const href = noteReferenceHref({ version: 1, kind: "task", id: "task" });
  const { links } = extractNoteEntityLinks(`See [[Plan]] and [Task](${href}); [nested [label]](${href})`);
  expect(links.map((link) => link.label)).toEqual(["Task", "nested [label]"]);
});

it("handles long unmatched punctuation before a real link without repeated scans", () => {
  const href = noteReferenceHref({ version: 1, kind: "task", id: "task" });
  expect(extractNoteEntityLinks(`${"[a".repeat(10_000)} [Task](${href})`).links.map((link) => link.label)).toEqual(["Task"]);
});

it("keeps long HTML-like literal task text intact", () => { const text = "<x".repeat(10_000); expect(noteInlineText(text)).toBe(text); });
