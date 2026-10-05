import { describe, expect, it } from "vitest";
import { threadTextIndex } from "./threadTextIndex";

describe("threadTextIndex", () => {
  it("indexes decoded plain text and falls back to the HTML body", () => {
    const index = threadTextIndex([
      { bodyText: "Tom &amp; Jerry went home early", bodyHtml: "<p>ignored when text exists</p>" },
      { bodyText: "  ", bodyHtml: "<p>Only <b>rich</b> text here</p><script>never indexed words</script>" },
    ]);
    const prior = index.before(2);
    expect(prior.has("tom jerry went home")).toBe(true);
    expect(prior.has("tom amp jerry went")).toBe(false);
    expect(prior.has("ignored when text exists")).toBe(false);
    expect(prior.has("only rich text here")).toBe(true);
    expect(prior.has("never indexed words")).toBe(false);
  });
});
