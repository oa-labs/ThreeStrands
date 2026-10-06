import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { describe, it } from "node:test";
import { LICENSE_RTF, markdownToRtf, renderLicenseRtf } from "./license-rtf.mjs";

describe("installer license RTF", () => {
  it("is regenerated from LICENSE (run `node scripts/license-rtf.mjs`)", async () => {
    assert.equal(await readFile(LICENSE_RTF, "utf8"), await renderLicenseRtf());
  });

  it("is the license file the macOS installer shows", async () => {
    const config = JSON.parse(await readFile(new URL("../src-tauri/tauri.macos.conf.json", import.meta.url), "utf8"));
    assert.equal(config.bundle.licenseFile, "LICENSE.rtf");
  });

  it("formats headings, emphasis, quotes, and links instead of showing Markdown syntax", () => {
    const rtf = markdownToRtf(
      "# Title\n\n## Section\n\nSee [Other](#other) at <https://example.com>.\n\n> Quoted `code`\n\n***Loud*** and **bold** text.\n",
    );
    assert.match(rtf, /^\{\\rtf1/);
    assert.match(rtf, /\\b\\fs32 Title\\b0/);
    assert.match(rtf, /\\b\\fs26 Section\\b0/);
    assert.match(rtf, /See Other at https:\/\/example\.com\./);
    assert.match(rtf, /\\li360\\sa180 \{\\i Quoted \{\\f1 code\}\}/);
    assert.match(rtf, /\{\\b\\i Loud\} and \{\\b bold\} text\./);
    assert.doesNotMatch(rtf, /[#*`]|\]\(|<https/);
  });

  it("escapes RTF control characters and non-ASCII text", () => {
    const rtf = markdownToRtf("Back\\slash {brace} © 😀");
    assert.match(rtf, /Back\\\\slash \\\{brace\\\} \\u169\? \\u-10179\?\\u-8704\?/);
  });

  it("joins wrapped lines within a paragraph", () => {
    assert.match(markdownToRtf("one\ntwo"), /\\pard\\sa180 one two\\par/);
  });
});
