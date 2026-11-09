import { describe, expect, it } from "vitest";
import { NOTE_PREVIEW_CHARS, notePlainText, notePreview } from "./notePreview";

// The same cases as `plain_text` in src-tauri/src/notes.rs.
describe("note previews", () => {
  it("drop Markdown syntax", () => {
    const body = "---\ntitle: x\n---\n## The Basics\n- **Dates (LOCKED):** Friday\n  * _Lodging_ ~~old~~ ==new== `code` see [VRBO](https://vrbo.com) <https://a.b>\n> quoted\n1. [ ] task [[Ada Lovelace|Ada]] [[Plan]] ![photo](x.png)\n---\n```js\nlet snake_case = 1;\n```\n| a | b |\n| --- | :-: |\n| 1 | 2 |\n\\*literal\\* 3 * 4";
    expect(notePlainText(body)).toBe("The Basics Dates (LOCKED): Friday Lodging old new code see VRBO https://a.b quoted task Ada Plan photo let snake_case = 1; a b 1 2 *literal* 3 4");
    expect(notePlainText("#hashtag and # heading")).toBe("#hashtag and # heading");
    expect(notePlainText("")).toBe("");
  });

  it("cut at the preview length", () => {
    const below = "a".repeat(NOTE_PREVIEW_CHARS - 1);
    expect(notePreview(below)).toBe(below);
    expect(notePreview("a".repeat(NOTE_PREVIEW_CHARS))).toBe("a".repeat(NOTE_PREVIEW_CHARS));
    expect(notePreview("a".repeat(NOTE_PREVIEW_CHARS + 1))).toBe(`${"a".repeat(NOTE_PREVIEW_CHARS - 1)}…`);
  });
});
