import { describe, expect, it } from "vitest";
import { sanitizeStyleSheet } from "./emailStyleSheet";
import { EMAIL_CSS_LIMITS } from "./emailRenderingPolicy";
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

  it("keeps safe theme overrides and scopes them to the message root", () => {
    const css = sanitizeStyleSheet(`
      @media (prefers-color-scheme: dark) {
        .wrapper { background-color: #000000; color: #000000; display: block; }
      }
    `, "dark");
    expect(css).toContain('[data-email-root][data-theme="dark"] .wrapper');
    expect(css).not.toContain('[data-email-root] [data-email-root]');
    expect(css).toContain("background-color: #000000");
    expect(css).toContain("display: block");
  });

  it("resolves color-scheme media against the selected light theme", () => {
    const css = sanitizeStyleSheet(`
      @media (prefers-color-scheme: dark) { .copy { color: white; } }
      @media (prefers-color-scheme: light) { .copy { color: black; } }
    `, "light");
    expect(css).toContain('[data-email-root][data-theme="light"] .copy');
    expect(css).toContain("color: black");
    expect(css).not.toContain("color: white");
  });

  it("drops active/resource at-rules while keeping safe responsive media queries", () => {
    const css = sanitizeStyleSheet(`
      @import url(https://tracker.invalid/evil.css);
      @font-face { font-family: "Evil"; src: url(https://tracker.invalid/evil.woff); }
      @keyframes flash { from { opacity: 0; } to { opacity: 1; } }
      @supports (display: grid) { .x { color: red; } }
      @media (min-width: 100px) { .x { color: red; } }
      @media screen { .x { color: blue; } }
    `);
    expect(css).toContain("@media (min-width: 100px)");
    expect(css).toContain("@media screen");
    expect(css).toContain("color: red");
    expect(css).not.toContain("@font-face");
    expect(css).not.toContain("@import");
    expect(css).not.toContain("@keyframes");
    expect(css).not.toContain("@supports");
  });

  it("drops ambiguous mixed themed media branches", () => {
    expect(sanitizeStyleSheet("@media (prefers-color-scheme: dark), screen { .x { color: red; } }", "dark")).toBe("");
  });

  it("parses selector lists, rewrites global roots, and scopes every selector", () => {
    const css = sanitizeStyleSheet(`
      body, .safe-one { color: red; }
      html { color: red; }
      :root { color: red; }
      * { color: red; }
      * > .also-unsafe { color: red; }
      .safe-two, div.safe-three { color: blue; }
    `);
    expect(css).toContain("[data-email-root]");
    expect(css).toContain("[data-email-root] .safe-one");
    expect(css).toContain("[data-email-root] .safe-two");
    expect(css).toContain("[data-email-root] div.safe-three");
    expect(css).not.toMatch(/\bbody\b/);
    expect(css).not.toMatch(/\bhtml\b/);
    expect(css).not.toMatch(/:root/);
    expect(css).toContain("[data-email-root] *");
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
    expect(css).not.toContain("position");
    expect(css).toContain("top: 0");
    expect(css).not.toContain("animation");
    expect(css).not.toContain("cursor");
    expect(css).not.toContain("content:");
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
    expect(css).not.toContain("<");
    // The rule itself survives; only the raw-text breakout is neutralized.
    expect(css).toMatch(/^\[data-email-root\] \[title="[^"]*&lt;script>[^"]*"\] \{ color: red; \}$/);
  });

  describe("media-query width bound (EMAIL_CSS_LIMITS.maxAbsolutePx)", () => {
    const limit = EMAIL_CSS_LIMITS.maxAbsolutePx;

    it.each(["min-width", "max-width"])("keeps %s below and at the limit and drops it above", (feature) => {
      for (const width of [limit - 1, limit]) {
        const css = sanitizeStyleSheet(`@media (${feature}: ${width}px) { .x { color: red; } }`);
        expect(css).toContain(`@media (${feature}: ${width}px)`);
        expect(css).toContain("color: red");
      }
      expect(sanitizeStyleSheet(`@media (${feature}: ${limit + 1}px) { .x { color: red; } }`)).toBe("");
    });

    it("drops the whole query when any and-joined or comma-listed width is above the limit", () => {
      expect(sanitizeStyleSheet(`@media screen and (min-width: ${limit}px) { .x { color: red; } }`)).toContain(`(min-width: ${limit}px)`);
      expect(sanitizeStyleSheet(`@media screen and (min-width: 100px) and (max-width: ${limit + 1}px) { .x { color: red; } }`)).toBe("");
      expect(sanitizeStyleSheet(`@media (max-width: 600px), (min-width: ${limit + 1}px) { .x { color: red; } }`)).toBe("");
    });
  });
});

describe("extractSafeStyleSheet + sanitizeMessageHtml integration", () => {
  it("supports a stylesheet-driven dark-mode image swap end to end", () => {
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

  it("supports a stylesheet-driven dark-mode table-row swap scoped to the selected theme", () => {
    // Structurally different from the image swap: the swapped elements are
    // table rows carrying text, and the reader-selected theme (not the OS)
    // decides which branch applies.
    const html = `
      <style>
        .dark-row { display: none; }
        @media (prefers-color-scheme: dark) {
          .light-row { display: none; }
          .dark-row { display: table-row; background-color: #111111; color: #eeeeee; }
        }
      </style>
      <table role="presentation">
        <tr class="light-row"><td>Light banner</td></tr>
        <tr class="dark-row"><td>Dark banner</td></tr>
      </table>
    `;
    const dark = extractSafeStyleSheet(html, "dark");
    expect(dark).toContain('[data-email-root][data-theme="dark"] .dark-row');
    expect(dark).toContain("display: table-row");
    expect(dark).not.toContain("prefers-color-scheme");

    const light = extractSafeStyleSheet(html, "light");
    expect(light).toContain("[data-email-root] .dark-row");
    expect(light).not.toContain("display: table-row");

    const body = sanitizeMessageHtml(html);
    expect(body).toContain('class="light-row"');
    expect(body).toContain('class="dark-row"');
    expect(body).not.toContain("<style");
  });

  it("produces no stylesheet when the message has none", () => {
    expect(extractSafeStyleSheet("<p>Hello</p>")).toBe("");
  });
});
