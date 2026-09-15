import { describe, expect, it } from "vitest";
import { sanitizeStyleSheet } from "./emailStyleSheet";
import { extractSafeStyleSheet, sanitizeMessageHtml } from "./SafeMessage";

describe("sanitizeStyleSheet", () => {
  it("keeps an allowlisted property/value under a media query and a class selector", () => {
    const css = sanitizeStyleSheet(`
      .dark-logo { display: none; }
      @media (prefers-color-scheme: dark) {
        .light-logo { display: none; }
        .dark-logo { display: inline-block; }
      }
    `);
    expect(css).toContain("@media (prefers-color-scheme: dark)");
    expect(css).toContain(".dark-logo");
    expect(css).toContain("display: inline-block");
  });

  it("preserves basic floats used to separate email action links", () => {
    const css = sanitizeStyleSheet(".open-action { float: right; }");
    expect(css).toContain("float: right");

    const body = sanitizeMessageHtml('<a href="https://example.com" style="float:right">Open</a>');
    expect(body).toContain("float: right");
  });

  it("drops color/background-color inside a prefers-color-scheme media query, even on an otherwise-safe selector", () => {
    // That media feature reflects the reader's real OS/webview appearance,
    // not this app's own theme, so letting a sender recolor text/backgrounds
    // there can silently produce unreadable (e.g. same-color-as-background)
    // text — see the comment on MEDIA_QUERY_SAFE_PROPERTIES in
    // emailStyleSheet.ts. Only the logo-swap use case (display/visibility)
    // is allowed in this context.
    const css = sanitizeStyleSheet(`
      @media (prefers-color-scheme: dark) {
        .wrapper { background-color: #000000; color: #000000; display: block; }
      }
    `);
    expect(css).toContain("display: block");
    expect(css).not.toContain("background-color");
    expect(css).not.toMatch(/(?<!background-)color: #000000/);
  });

  it("drops every other at-rule (@font-face, @import, @keyframes, @supports, unrelated @media)", () => {
    const css = sanitizeStyleSheet(`
      @import url(https://tracker.invalid/evil.css);
      @font-face { font-family: "Evil"; src: url(https://tracker.invalid/evil.woff); }
      @keyframes flash { from { opacity: 0; } to { opacity: 1; } }
      @supports (display: grid) { .x { color: red; } }
      @media (min-width: 100px) { .x { color: red; } }
      @media screen { .x { color: red; } }
    `);
    expect(css).toBe("");
  });

  it("drops selectors targeting html, body, :root, or a bare universal selector, keeping the rest of the list", () => {
    const css = sanitizeStyleSheet(`
      body, .safe-one { color: red; }
      html { color: red; }
      :root { color: red; }
      * { color: red; }
      * > .also-unsafe { color: red; }
      .safe-two, div.safe-three { color: blue; }
    `);
    expect(css).toContain(".safe-one");
    expect(css).toContain(".safe-two");
    expect(css).toContain("div.safe-three");
    expect(css).not.toMatch(/\bbody\b/);
    expect(css).not.toMatch(/\bhtml\b/);
    expect(css).not.toMatch(/:root/);
    expect(css).not.toContain("*");
  });

  it("never allows a property outside the shared safeStyles allowlist", () => {
    const css = sanitizeStyleSheet(`
      .x {
        position: fixed;
        top: 0;
        animation: spin 1s infinite;
        cursor: pointer;
        content: "injected";
        color: red;
      }
    `);
    expect(css).toContain("color: red");
    expect(css).not.toMatch(/position|top:|animation|cursor|content/);
  });

  it("drops any declaration whose value carries a url(), even on an otherwise-allowed property", () => {
    // background-image isn't in the shared allowlist to begin with, but this
    // guards the general case: a selector can match zero, one, or many
    // elements, so there's no single DOM node to gate through the image
    // proxy the way inline style="background-image:..." and <img src> both
    // are — so no property may carry url() here, full stop.
    const css = sanitizeStyleSheet('.x { background: url(https://tracker.invalid/pixel.gif); color: red; }');
    expect(css).toContain("color: red");
    expect(css).not.toContain("url(");
    expect(css).not.toContain("background");
  });

  it("returns an empty string for unparseable CSS instead of throwing", () => {
    expect(sanitizeStyleSheet("{{{ not css at all ]]]")).toBe("");
  });

  it("escapes a literal < so a crafted selector or value can't break out of the <style> tag", () => {
    // A quoted attribute-selector value is a free-form string as far as CSS
    // syntax is concerned — nothing about parsing it as a selector rejects
    // "</style><script>alert(1)</script>" appearing inside the quotes.
    const css = sanitizeStyleSheet('[title="</style><script>alert(1)<\\/script>"] { color: red; }');
    expect(css).not.toContain("</style");
    expect(css).not.toContain("<script");
    if (css) expect(css).toContain("&lt;");
  });
});

describe("extractSafeStyleSheet + sanitizeMessageHtml integration", () => {
  it("supports the Gmail dark-mode logo-swap pattern end to end", () => {
    const html = `
      <style>
        .light-logo { display: inline-block; }
        .dark-logo { display: none; }
        @media (prefers-color-scheme: dark) {
          .light-logo { display: none; }
          .dark-logo { display: inline-block; }
        }
      </style>
      <img class="light-logo" src="https://example.com/logo-light.png">
      <img class="dark-logo" src="https://example.com/logo-dark.png">
    `;
    const styleSheet = extractSafeStyleSheet(html);
    expect(styleSheet).toContain("@media (prefers-color-scheme: dark)");
    expect(styleSheet).toContain(".dark-logo");

    const body = sanitizeMessageHtml(html);
    expect(body).toContain('class="light-logo"');
    expect(body).toContain('class="dark-logo"');
    // <style> tags themselves are still stripped from the body by
    // DOMPurify — extractSafeStyleSheet pulls them from the original html
    // independently and the caller places the result in <head> instead.
    expect(body).not.toContain("<style");
  });

  it("produces no stylesheet when the message has none", () => {
    expect(extractSafeStyleSheet("<p>Hello</p>")).toBe("");
  });
});
