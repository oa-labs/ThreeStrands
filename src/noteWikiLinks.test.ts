import { describe, expect, it } from "vitest";
import { extractWikiLinks, noteTitleKey, rewriteWikiLinks, splitWikiLink } from "./noteWikiLinks";

// The same cases as the native parser's tests in src-tauri/src/notes.rs.
describe("wiki links", () => {
  it("finds links with shown text and skips code, escapes, and malformed ones", () => {
    const body = "See [[Project  plan]] and [[Ada|her]].\n`[[Code]]` \\[[Escaped]] [[a[b]] [[]]\n```\n[[Fenced]]\n```\nEnd [[Last]]";
    expect(extractWikiLinks(body)).toEqual([
      { title: "Project plan", context: "See [[Project  plan]] and [[Ada|her]]." },
      { title: "Ada", context: "See [[Project  plan]] and [[Ada|her]]." },
      { title: "Last", context: "End [[Last]]" },
    ]);
  });

  it("points links at a renamed title and keeps shown text, code, and line endings", () => {
    const body = "See [[Project  plan]] and [[project plan|the plan]].\r\nNot [[Other]] or `[[Project plan]]`, nor \\[[Project plan]].\n```\n[[Project plan]]\n```\nLast [[PROJECT PLAN]]";
    expect(rewriteWikiLinks(body, noteTitleKey("Project plan"), "Roadmap")).toBe(
      "See [[Roadmap]] and [[Roadmap|the plan]].\r\nNot [[Other]] or `[[Project plan]]`, nor \\[[Project plan]].\n```\n[[Project plan]]\n```\nLast [[Roadmap]]",
    );
    expect(rewriteWikiLinks(body, noteTitleKey("Nothing"), "Roadmap")).toBeNull();
  });

  it("splits a link into its target and shown text", () => {
    expect(splitWikiLink("Ada  Lovelace")).toEqual({ target: "Ada Lovelace", label: "Ada Lovelace" });
    expect(splitWikiLink("Ada Lovelace|Ada")).toEqual({ target: "Ada Lovelace", label: "Ada" });
    expect(splitWikiLink("Ada|")).toEqual({ target: "Ada", label: "Ada" });
  });
});
