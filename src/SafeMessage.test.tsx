import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  SafeMessage,
  fillResolvedImages,
  collapseQuotedHistoryHtml,
  collapseQuotedHistoryText,
  extractBlockedImageUrls,
  extractSafeStyleSheet,
  fitsMessageImageBudget,
  linkifyText,
  sanitizeMessageHtml,
} from "./SafeMessage";
import { EMAIL_CSS_LIMITS, EMAIL_IMAGE_LIMITS } from "./emailRenderingPolicy";
import { emailRenderingFixtures } from "./test/emailRenderingFixtures";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

afterEach(() => {
  cleanup();
  vi.mocked(openUrl).mockClear();
});

/**
 * jsdom does not populate an iframe from srcdoc. Mirror the document into the
 * frame and fire load, as a browser would, so tests observe the live frame
 * the way a reader sees it.
 */
function loadFrame(frame: HTMLIFrameElement) {
  frame.contentDocument!.body.innerHTML = new DOMParser().parseFromString(frame.srcdoc, "text/html").body.innerHTML;
  fireEvent.load(frame);
  return frame.contentDocument!;
}

describe("SafeMessage", () => {
  it("preserves structurally distinct notification and transactional layouts", () => {
    expect(sanitizeMessageHtml(emailRenderingFixtures.notification)).toContain("<table");
    expect(sanitizeMessageHtml(emailRenderingFixtures.transactional)).toContain('class="layout"');
    const stylesheet = extractSafeStyleSheet(emailRenderingFixtures.transactional, "dark");
    expect(stylesheet).toContain("@media screen and (max-width:600px)");
    expect(stylesheet).toContain('[data-email-root][data-theme="dark"] .dark-copy');
  });

  const parseSanitized = (fixture: keyof typeof emailRenderingFixtures) =>
    new DOMParser().parseFromString(sanitizeMessageHtml(emailRenderingFixtures[fixture]), "text/html").body;

  it("keeps the newsletter fixture's table spacing and width cap", () => {
    const table = parseSanitized("newsletter").querySelector("table")!;
    expect(table.style.maxWidth).toBe("640px");
    expect(table.style.borderSpacing).toBe("8px");
    expect(table.querySelector("td")!.style.padding).toBe("16px");
  });

  it("keeps the table fixture's presentational attributes", () => {
    const table = parseSanitized("table").querySelector("table")!;
    expect(table.getAttribute("cellpadding")).toBe("12");
    expect(table.getAttribute("cellspacing")).toBe("4");
    expect(table.querySelector("th")!.getAttribute("align")).toBe("left");
    expect(table.querySelector("td[align='right']")?.textContent).toBe("$24");
  });

  it("keeps the flex fixture's flow layout", () => {
    const row = parseSanitized("flex").querySelector("div")!;
    expect(row.style.display).toBe("flex");
    expect(row.style.gap).toBe("8px");
    expect(row.style.justifyContent).toBe("space-between");
    expect(row.querySelector("span")!.style.flex).toBe("1 1 180px");
  });

  it("keeps the darkMode fixture's themed rule and the element it targets", () => {
    expect(parseSanitized("darkMode").querySelector("div.panel")?.textContent).toBe("Theme-aware content");
    expect(extractSafeStyleSheet(emailRenderingFixtures.darkMode, "dark")).toContain(".panel");
  });

  it("keeps the spacer fixture's empty structural elements", () => {
    const body = parseSanitized("spacer");
    expect(body.querySelector("div:empty")).not.toBeNull();
    expect(body.querySelector("td:empty")).not.toBeNull();
    expect(body.innerHTML).toContain("&nbsp;");
  });

  it("enforces the capability boundary on malformed fixture content", () => {
    const sanitized = sanitizeMessageHtml(emailRenderingFixtures.malformed);
    expect(sanitized).not.toContain("position");
    expect(sanitized).not.toContain("animation");
    expect(sanitized).not.toContain("<form");
    expect(sanitized).not.toContain("<img");
  });

  it("removes active content and always parks remote images behind a blocked-src marker", () => {
    const sanitized = sanitizeMessageHtml(`
      <p style="background:url(https://tracker.invalid)">Hello</p>
      <img src="https://tracker.invalid/open.gif" />
      <form action="https://attacker.invalid"><input name="token" /></form>
      <script>alert("bad")</script>
    `);

    expect(sanitized).toContain("Hello");
    // A background-image is as much a network request as <img src>, so it's
    // held behind the same blocked-src marker instead of a live style —
    // resolving it is the caller's job (see applyResolvedImages), never
    // something sanitizeMessageHtml itself does.
    expect(sanitized).not.toMatch(/style="[^"]*url\(/);
    expect(sanitized).toContain('data-blocked-src="https://tracker.invalid"');
    expect(sanitized).toContain("<img");
    expect(sanitized).not.toContain(' src="');
    expect(sanitized).toContain('data-blocked-src="https://tracker.invalid/open.gif"');
    expect(sanitized).not.toContain("<form");
    expect(sanitized).not.toContain("<script");
  });

  it("drops images with unsafe or non-image src values", () => {
    const sanitized = sanitizeMessageHtml('<img src="javascript:alert(1)" /><img src="file:///etc/passwd" />');
    expect(sanitized).not.toContain("<img");
  });

  it("preserves harmless empty elements and sender-authored spacing", () => {
    const sanitized = sanitizeMessageHtml(`
      <div style="padding:40px 0"></div>
      <p>&nbsp;</p>
      <div><div style="margin-bottom:60px"><span></span></div></div>
      <table><tr><td style="padding:200px">&nbsp;</td><td>Real content</td></tr></table>
      <p>Hello</p>
    `);

    expect(sanitized).toContain("padding-top: 40px");
    expect(sanitized).toContain("margin-bottom: 60px");
    expect(sanitized).toContain("padding-top: 200px");
    expect(sanitized).toContain("&nbsp;");
    expect(sanitized).toContain("Real content");
    expect(sanitized).toContain("Hello");
  });

  it("keeps an empty div that paints a background instead of treating it as a dead spacer", () => {
    // A colored, childless div with no text (a status dot, a swatch, a
    // divider bar) is visually indistinguishable from a dead spacer div by
    // shape alone — the two are only told apart by whether it actually
    // paints something.
    const sanitized = sanitizeMessageHtml(`
      <div style="padding:40px 0"></div>
      <div style="background-color:#ed353b;border-radius:50%;width:8px;height:8px"></div>
    `);
    expect(sanitized).toContain("padding-top: 40px");
    expect(sanitized).toContain("background-color: rgb(237, 53, 59)");
    expect(sanitized).toContain("width: 8px");
    expect(sanitized).toContain("height: 8px");
  });

  it("preserves empty spacers while dropping only out-of-policy declarations", () => {
    const sanitized = sanitizeMessageHtml(`
      <div style="height:18px"></div>
      <div style="height:20px"></div>
      <div style="height:200px"></div>
      <div style="height:5000px"></div>
      <div style="height:50%"></div>
    `);
    const container = document.createElement("div");
    container.innerHTML = sanitized;

    expect(Array.from(container.children).map((element) => (element as HTMLElement).style.height)).toEqual(["18px", "20px", "200px", "", "50%"]);
  });

  it("preserves native table spacing without synthesizing cell styles", () => {
    const sanitized = sanitizeMessageHtml(`
      <table cellpadding="0" cellspacing="0">
        <tr>
          <td style="padding-right:4px">A</td>
          <td>B</td>
        </tr>
      </table>
    `);
    const container = document.createElement("div");
    container.innerHTML = sanitized;
    const [tdA, tdB] = Array.from(container.querySelectorAll("td"));
    expect(tdA.style.paddingRight).toBe("4px");
    expect(tdA.style.paddingTop).toBe("");
    expect(tdA.style.paddingLeft).toBe("");
    expect(tdB.style.padding).toBe("");
    expect(container.querySelector("table")?.getAttribute("cellpadding")).toBe("0");
  });

  it("keeps a sender's own margin:0 heading/paragraph reset from a <style> block", () => {
    // Stylesheet declarations use the same policy as inline declarations, so
    // sender resets remain available without any ThreeStrands geometry override.
    const styleSheet = extractSafeStyleSheet("<style>h1, p { margin: 0; }</style>");
    expect(styleSheet).toContain("margin: 0");
  });

  it("keeps a lone &nbsp; paragraph as a blank-line spacer instead of collapsing it away", () => {
    // Marketing templates commonly zero every <p>'s margin and rely on a
    // standalone "<p>&nbsp;</p>" to reserve a blank line's height between
    // sections. JS's \s matches U+00A0, so this used to be misidentified as
    // an empty wrapper (like an indentation-only "<p>\n</p>") and removed
    // outright, collapsing sections together that every other mail client
    // renders with visible spacing between them.
    const sanitized = sanitizeMessageHtml(`
      <p style="margin:0">What's changing</p>
      <p style="margin:0">&nbsp;</p>
      <p style="margin:0"></p>
      <p style="margin:0">Why we're making this change</p>
    `);

    expect(sanitized).toContain("&nbsp;");
    expect(sanitized.match(/<p/g)).toHaveLength(4);
  });

  it("preserves a zero margin/padding given in em/rem/%, not just px, and still caps large values per unit", () => {
    // A sender zeroing out a browser default (e.g. a <p>'s ~1em margin) in
    // the same unit as their own font-size is as common as doing it in px.
    // Dropping "margin-bottom: 0em" let the UA default margin resurface
    // instead of the zero the sender asked for.
    const sanitized = sanitizeMessageHtml(`
      <p style="margin-top:0px;margin-bottom:0em;">Tight</p>
      <div style="padding-top:1.5em;padding-left:200%;">Padded</div>
    `);
    expect(sanitized).toContain("margin-bottom: 0em");
    expect(sanitized).toContain("padding-top: 1.5em");
    // Out-of-policy values are dropped rather than silently rewritten.
    expect(sanitized).not.toContain("padding-left");
  });

  it("keeps newsletter preheaders hidden and preserves safe email dimensions", () => {
    const sanitized = sanitizeMessageHtml(`
      <div class="preview" style="display:none;font-size:1px;max-height:0;overflow:hidden">
        The summer heat is slowly making its retreat.
      </div>
      <table width="100%"><tr><td width="550">
        <img src="https://example.com/avatar.gif" width="40" height="40" style="width:40px;height:40px">
        <img src="https://example.com/hero.png" width="550" height="183.207">
      </td></tr></table>
    `);
    const container = document.createElement("div");
    container.innerHTML = sanitized;

    expect(container.querySelector("div")?.hidden).toBe(true);
    expect(container.querySelector("table")?.getAttribute("width")).toBe("100%");
    expect(container.querySelector("td")?.getAttribute("width")).toBe("550");
    expect(container.querySelectorAll("img")[0].getAttribute("width")).toBe("40");
    expect(container.querySelectorAll("img")[0].getAttribute("height")).toBe("40");
    expect(container.querySelectorAll("img")[1].getAttribute("width")).toBe("550");
    expect(container.querySelectorAll("img")[1].getAttribute("height")).toBe("183.207");
  });

  it("rejects unsafe or layout-breaking dimensions", () => {
    const sanitized = sanitizeMessageHtml(`
      <div width="500">Text</div>
      <table width="120%"><tr><td width="100000"><img src="https://example.com/a.png" width="calc(100vw)" height="100%"></td></tr></table>
    `);
    const container = document.createElement("div");
    container.innerHTML = sanitized;

    expect(container.querySelector("div")?.hasAttribute("width")).toBe(false);
    expect(container.querySelector("table")?.hasAttribute("width")).toBe(false);
    expect(container.querySelector("td")?.hasAttribute("width")).toBe(false);
    expect(container.querySelector("img")?.hasAttribute("width")).toBe(false);
    expect(container.querySelector("img")?.hasAttribute("height")).toBe(false);
  });

  it("renders safe message formatting inside a sandboxed, CSP-scoped iframe that only allows data: images", () => {
    render(<SafeMessage html="<p>Hello <strong>friend</strong></p>" />);
    const frame = screen.getByTestId("message-body") as HTMLIFrameElement;
    expect(frame.tagName).toBe("IFRAME");
    expect(frame.getAttribute("sandbox")).toBe("allow-same-origin allow-scripts");
    expect(frame.srcdoc).toContain("<p>Hello <strong>friend</strong></p>");
    expect(frame.getAttribute("referrerpolicy")).toBe("no-referrer");
    // The sandbox grants allow-scripts (see SafeMessage), so this CSP is the
    // only thing stopping sender script: pin every directive, not a sample.
    const csp = new DOMParser()
      .parseFromString(frame.srcdoc, "text/html")
      .querySelector('meta[http-equiv="Content-Security-Policy"]')
      ?.getAttribute("content");
    expect(csp?.split(";").map((directive) => directive.trim()).sort()).toEqual([
      "base-uri 'none'",
      "default-src 'none'",
      "form-action 'none'",
      "frame-src 'none'",
      "img-src data:",
      "object-src 'none'",
      "script-src 'none'",
      "style-src 'unsafe-inline'",
    ]);
    // break-word, not anywhere: anywhere shrinks a box's minimum content
    // size for auto-layout, so a narrow fixed-width table cell (a numbered
    // list's index column, say) would treat even a short 2-character
    // string as breakable and split it across lines instead of letting the
    // column render slightly wider than its width hint.
    expect(frame.srcdoc).toContain("overflow-wrap: break-word");
    expect(frame.srcdoc).not.toContain("overflow-wrap: anywhere");
    expect(frame.srcdoc).toContain("text-decoration-skip-ink: none");
  });

  it.each([
    ["sender-authored link", '<p>Visit <a href="https://example.com/paging">paging</a></p>'],
    ["auto-linkified plain text", "https://example.com/paging"],
  ])("keeps link underlines continuous for a %s", (_case, html) => {
    render(<SafeMessage html={html} />);
    const frame = screen.getByTestId("message-body") as HTMLIFrameElement;

    expect(frame.srcdoc).toContain("text-decoration-skip-ink: none");
    expect(frame.srcdoc).toContain("script-src 'none'");
    expect(frame.srcdoc).toContain("img-src data:");
    const body = new DOMParser().parseFromString(frame.srcdoc, "text/html").body;
    expect(body.querySelectorAll('a[href="https://example.com/paging"]')).toHaveLength(1);
  });

  it("safely embeds an installed font family name in the message document", () => {
    render(<SafeMessage html="<p>Hello</p>" fontFamily={'Font"; color: red; "'} />);
    const frame = screen.getByTestId("message-body") as HTMLIFrameElement;
    const document = new DOMParser().parseFromString(frame.srcdoc, "text/html");

    expect(document.body.style.fontFamily).toContain('Font\\"; color: red; \\"');
    expect(document.body.style.color).toBe("");
  });

  it("forwards keyboard events from the message iframe to the application window", () => {
    render(<SafeMessage html="<p>Hello friend</p>" />);
    const frame = screen.getByTestId("message-body") as HTMLIFrameElement;
    fireEvent.load(frame);
    const shortcut = vi.fn((event: KeyboardEvent) => event.preventDefault());
    window.addEventListener("keydown", shortcut);

    const accepted = fireEvent.keyDown(frame.contentDocument!.body, {
      key: "K",
      code: "KeyK",
      metaKey: true,
      shiftKey: true,
    });

    expect(shortcut).toHaveBeenCalledTimes(1);
    expect(shortcut.mock.calls[0][0]).toMatchObject({
      key: "K",
      code: "KeyK",
      metaKey: true,
      shiftKey: true,
    });
    expect(accepted).toBe(false);
    window.removeEventListener("keydown", shortcut);
  });

  it.each([
    {
      name: "wrapped reply opener and blockquote",
      html: `
        <div>My current reply</div>
        <div>
          <div>On Friday, A. Sender wrote:</div>
          <blockquote>Earlier message</blockquote>
        </div>
      `,
      current: "My current reply",
      quoted: "Earlier message",
    },
    {
      name: "forwarded-message separator and header block",
      html: emailRenderingFixtures.forwarded,
      current: "FYI, see below.",
      quoted: "Original details.",
    },
  ])("collapses a $name behind an ellipsis until clicked", ({ html, current, quoted }) => {
    render(<SafeMessage html={html} />);

    const frame = screen.getByTestId("message-body") as HTMLIFrameElement;
    expect(frame.srcdoc).toContain(current);
    expect(frame.srcdoc).not.toContain(quoted);

    fireEvent.click(screen.getByRole("button", { name: "Show quoted content" }));
    expect(frame.srcdoc).toContain(quoted);
    expect(screen.queryByRole("button", { name: "Show quoted content" })).not.toBeInTheDocument();
  });

  it("collapses generic original-message separators while preserving the current HTML", () => {
    const collapsed = collapseQuotedHistoryHtml(`
      <p>Current answer</p>
      <div>------ Original Message ------</div>
      <div>From A. Sender</div>
    `);

    expect(collapsed).toContain("Current answer");
    expect(collapsed).not.toContain("Original Message");
    expect(collapsed).not.toContain("From A. Sender");
  });

  it("does not hide a blockquote when it is the only message content", () => {
    expect(collapseQuotedHistoryHtml("<blockquote>A standalone quotation</blockquote>")).toBeNull();
  });

  it("folds semantic reply and forwarded markers without provider selectors", () => {
    const reply = collapseQuotedHistoryHtml(emailRenderingFixtures.reply);
    expect(reply).toContain("Here is my answer.");
    expect(reply).not.toContain("Earlier message content");

    const forwarded = collapseQuotedHistoryHtml(emailRenderingFixtures.forwarded);
    expect(forwarded).toContain("FYI, see below.");
    expect(forwarded).not.toContain("Original details.");
  });

  it("folds an 'On ... wrote:' opener whose 'wrote:' hard-wrapped onto its own line or node", () => {
    const acrossLineBreak = collapseQuotedHistoryHtml(emailRenderingFixtures.replyWrappedWroteLineBreak);
    expect(acrossLineBreak).toContain("Here is my answer.");
    expect(acrossLineBreak).not.toContain("Earlier message content");

    const acrossParagraphs = collapseQuotedHistoryHtml(emailRenderingFixtures.replyWrappedWroteParagraphs);
    expect(acrossParagraphs).toContain("Here is my answer.");
    expect(acrossParagraphs).not.toContain("Earlier message content");
  });

  it("keeps ambiguous quoted prose visible", () => {
    const html = "<p>My answer includes a quotation:</p><blockquote><p>Important cited text.</p></blockquote>";
    expect(collapseQuotedHistoryHtml(html)).toBeNull();
  });

  it("folds a trailing header-and-quote cluster without relying on provider markup", () => {
    const html = `<p>Current answer.</p><div><div>From: sender@example.com<br>Date: Tue, Sep 15, 2026<br>Subject: Details</div><blockquote>Earlier details.</blockquote></div>`;
    expect(collapseQuotedHistoryHtml(html)).toContain("Current answer.");
    expect(collapseQuotedHistoryHtml(html)).not.toContain("Earlier details.");
  });
});

it("preserves safe formatting and the class attribute while removing CSS requests and positioning", () => {
  const sanitized = sanitizeMessageHtml('<table class="modal"><tr><td style="text-align:center;font-weight:700;padding:200px;background-image:url(https://tracker.invalid);position:fixed;color:black">Invoice</td></tr></table>');
  expect(sanitized).toContain("text-align: center");
  expect(sanitized).toContain("font-weight: 700");
  expect(sanitized).toContain("padding-top: 200px");
  expect(sanitized).toContain("color: black");
  // class survives now that <style> blocks can target it (see
  // emailStyleSheet.ts) — it carries no special handling on its own.
  expect(sanitized).toContain('class="modal"');
  // background-image is gated the same way as <img src> rather than stripped
  // outright, so its URL only survives as an inert blocked-src marker.
  expect(sanitized).toContain('data-blocked-src="https://tracker.invalid"');
  expect(sanitized).not.toMatch(/style="[^"]*url\(|position/);
});

it("extracts every distinct blocked-src URL, deduplicated", () => {
  const sanitized = sanitizeMessageHtml(`
    <img src="https://example.com/a.png">
    <img src="https://example.com/a.png">
    <div style="background-image:url(https://example.com/b.png)">Hi</div>
  `);
  expect(extractBlockedImageUrls(sanitized).sort()).toEqual([
    "https://example.com/a.png",
    "https://example.com/b.png",
  ]);
});

it("fillResolvedImages fills in an <img> src and a background-image once resolved, leaving unresolved ones blocked", () => {
  const sanitized = sanitizeMessageHtml(`
    <img src="https://example.com/a.png">
    <div style="background-image:url(https://example.com/b.png)">Hi</div>
    <img src="https://example.com/still-pending.png">
  `);
  const resolved = new Map([
    ["https://example.com/a.png", "data:image/png;base64,AAA="],
    ["https://example.com/b.png", "data:image/png;base64,BBB="],
  ]);
  const container = document.createElement("div");
  container.innerHTML = sanitized;
  fillResolvedImages(container, resolved);

  expect(container.querySelectorAll("img")[0].getAttribute("src")).toBe("data:image/png;base64,AAA=");
  expect(container.querySelector("div")?.style.backgroundImage).toBe('url("data:image/png;base64,BBB=")');
  expect(container.querySelectorAll("img")[1].hasAttribute("src")).toBe(false);
  expect(container.querySelectorAll("img")[1].getAttribute("data-blocked-src")).toBe("https://example.com/still-pending.png");
});

it("fillResolvedImages never admits a resolved value that is not a data: URI", () => {
  const sanitized = sanitizeMessageHtml(`
    <img src="https://example.com/a.png">
    <div style="background-image:url(https://example.com/b.png)">Hi</div>
  `);
  const container = document.createElement("div");
  container.innerHTML = sanitized;
  fillResolvedImages(container, new Map([
    ["https://example.com/a.png", "https://tracker.invalid/pixel.gif"],
    ["https://example.com/b.png", "javascript:alert(1)"],
  ]));

  expect(container.querySelector("img")?.hasAttribute("src")).toBe(false);
  expect(container.querySelector("img")?.getAttribute("data-blocked-src")).toBe("https://example.com/a.png");
  expect(container.querySelector("div")?.style.backgroundImage).toBe("");
  expect(container.querySelector("div")?.getAttribute("data-blocked-src")).toBe("https://example.com/b.png");
});

it("drops a background-image whose URL fails the same scheme validation as <img src>", () => {
  // Parking a safe https: background behind data-blocked-src is covered by
  // the "removes active content" test above; this pins the negative side.
  for (const url of ["javascript:alert(1)", "file:///etc/passwd"]) {
    const unsafe = sanitizeMessageHtml(`<div style="background-image:url(${url})">Hi</div>`);
    expect(unsafe).toContain("Hi");
    expect(unsafe).not.toContain("background-image");
    expect(unsafe).not.toContain("data-blocked-src");
  }
});

it("preserves line-height, borders, bgcolor, cellpadding/cellspacing, and CSS width/height", () => {
  const sanitized = sanitizeMessageHtml(`
    <table bgcolor="#fff" cellpadding="8" cellspacing="999" height="120" style="width:100%">
      <tr>
        <td style="line-height:1.5;border-bottom:1px solid #ddd;width:50%">Row</td>
      </tr>
    </table>
    <img src="https://example.com/a.png" style="width:120px;height:9999px">
  `);
  const container = document.createElement("div");
  container.innerHTML = sanitized;

  const table = container.querySelector("table")!;
  expect(table.getAttribute("bgcolor")).toBe("#fff");
  expect(table.getAttribute("cellpadding")).toBe("8");
  expect(table.getAttribute("cellspacing")).toBe("999");
  expect(table.getAttribute("height")).toBe("120");
  expect(table.style.width).toBe("100%");

  const td = container.querySelector("td")!;
  expect(td.style.lineHeight).toBe("1.5");
  expect(td.style.borderBottom).toBe("1px solid rgb(221, 221, 221)");
  expect(td.style.width).toBe("50%");

  const img = container.querySelector("img")!;
  expect(img.style.width).toBe("120px");
  expect(img.style.height).toBe(""); // over the 4096px cap, dropped
});

it("preserves mix-blend-mode so layered blend compositing keeps canceling out", () => {
  // Senders can pair a black background with screen+difference blend modes
  // so that a client-side color inversion cancels itself out; both modes are
  // a no-op against black, so without mix-blend-mode the black background has
  // nothing to cancel it out and renders as an opaque block over the text.
  // mix-blend-mode is a cosmetic keyword-only property: it cannot fetch or
  // position anything, so it must survive on every sender-authored path.
  const sanitized = sanitizeMessageHtml(
    '<div style="background:#000;mix-blend-mode:screen"><div style="background:#000;mix-blend-mode:difference"><p>Hi</p></div></div>',
  );
  const container = document.createElement("div");
  container.innerHTML = sanitized;
  const [outer, inner] = Array.from(container.querySelectorAll("div"));
  expect(outer.style.mixBlendMode).toBe("screen");
  expect(inner.style.mixBlendMode).toBe("difference");

  // Same invariant through a class-targeted stylesheet on table cells.
  const tableHtml = `
    <style>
      .blend-outer { background-color: #000000; mix-blend-mode: screen; }
      .blend-inner { background-color: #000000; mix-blend-mode: difference; }
    </style>
    <table role="presentation"><tr><td class="blend-outer"><table><tr><td class="blend-inner">Hi</td></tr></table></td></tr></table>
  `;
  const styleSheet = extractSafeStyleSheet(tableHtml);
  expect(styleSheet).toMatch(/\[data-email-root\] \.blend-outer \{[^}]*mix-blend-mode: screen/);
  expect(styleSheet).toMatch(/\[data-email-root\] \.blend-inner \{[^}]*mix-blend-mode: difference/);
  expect(sanitizeMessageHtml(tableHtml)).toContain('class="blend-inner"');
  expect(extractSafeStyleSheet("<style>.x { mix-blend-mode: url(https://tracker.invalid/x); }</style>")).toBe("");
});

it("preserves text color", () => {
  const sanitized = sanitizeMessageHtml('<p style="color:#ffffff">Hi</p>');
  const container = document.createElement("div");
  container.innerHTML = sanitized;
  expect(container.querySelector("p")?.style.color).toBe("rgb(255, 255, 255)");
});

it("rejects unsafe border and bgcolor values", () => {
  const sanitized = sanitizeMessageHtml(
    '<table bgcolor="expression(alert(1))"><tr><td style="border:1px solid url(https://tracker.invalid)">Hi</td></tr></table>',
  );
  expect(sanitized).not.toContain("bgcolor");
  expect(sanitized).not.toContain("border");
});

it("renders plain text literally when HTML is absent or stripped", () => {
  render(<SafeMessage html="<script>alert(1)</script>" text={'Hello\n\n<script>literal text</script>'} />);
  const body = screen.getByTestId("message-body");
  expect(body.textContent).toBe('Hello\n\n<script>literal text</script>');
  expect(body.querySelector("script")).toBeNull();
  expect(body).toHaveClass("message-body-plain");
});

it("decodes entities in the plain text fallback", () => {
  render(<SafeMessage html="" text="Tom &#39;s message &amp; details" />);
  expect(screen.getByTestId("message-body")).toHaveTextContent("Tom 's message & details");
});

it("collapses and reveals quoted history in a plain-text reply", () => {
  const text = "Current answer\n\n------ Original Message ------\nFrom A. Sender\nEarlier message";
  expect(collapseQuotedHistoryText(text)).toBe("Current answer");
  render(<SafeMessage html="" text={text} />);

  expect(screen.getByTestId("message-body")).toHaveTextContent("Current answer");
  expect(screen.getByTestId("message-body")).not.toHaveTextContent("Earlier message");
  fireEvent.click(screen.getByRole("button", { name: "Show quoted content" }));
  expect(screen.getByTestId("message-body")).toHaveTextContent("Earlier message");
});

it("collapses an 'On ... wrote:' opener whose 'wrote:' hard-wrapped onto the next line", () => {
  const text = [
    "Please let me know if there are any other edits needed.",
    "",
    "On Mon, Sep 21, 2026 at 2:33 PM A. Sender <sender@example.com>",
    "wrote:",
    "",
    "> Yes, these are examples of several recurring issues.",
  ].join("\n");
  expect(collapseQuotedHistoryText(text)).toBe("Please let me know if there are any other edits needed.");
});

it("does not collapse fewer than 5 consecutive '>' lines", () => {
  const text = ["Current answer", "", "> line one", "> line two", "> line three"].join("\n");
  expect(collapseQuotedHistoryText(text)).toBeNull();
});

it("collapses at the first line of a 5+ line '>' quote run", () => {
  const text = [
    "Current answer",
    "",
    "> line one",
    "> line two",
    "> line three",
    "> line four",
    "> line five",
  ].join("\n");
  expect(collapseQuotedHistoryText(text)).toBe("Current answer");
});

it("linkifies bare URLs, www.-domains, and email addresses without swallowing trailing punctuation", () => {
  const nodes = linkifyText("See https://example.com/path, or www.example.org. Contact tom@example.com!");
  const { container } = render(<>{nodes}</>);
  const links = container.querySelectorAll("a");
  expect(Array.from(links).map((a) => [a.getAttribute("href"), a.textContent])).toEqual([
    ["https://example.com/path", "https://example.com/path"],
    ["https://www.example.org", "www.example.org"],
    ["mailto:tom@example.com", "tom@example.com"],
  ]);
  expect(container.textContent).toBe(
    "See https://example.com/path, or www.example.org. Contact tom@example.com!",
  );
});

describe("linkifying bare URLs in HTML bodies", () => {
  const anchorsIn = (html: string) =>
    Array.from(new DOMParser().parseFromString(sanitizeMessageHtml(html), "text/html").body.querySelectorAll("a"))
      .map((anchor) => [anchor.getAttribute("href"), anchor.textContent, anchor.getAttribute("rel")]);

  it.each([
    {
      name: "a URL alone in a paragraph",
      html: "<p>See https://example.com/paging now.</p>",
      expected: [["https://example.com/paging", "https://example.com/paging", "noopener noreferrer"]],
    },
    {
      name: "several links sharing one table-cell text node",
      html: "<table><tr><td>Docs: www.example.org, help@example.com or https://example.net/a?b=1</td></tr></table>",
      expected: [
        ["https://www.example.org", "www.example.org", "noopener noreferrer"],
        ["mailto:help@example.com", "help@example.com", "noopener noreferrer"],
        ["https://example.net/a?b=1", "https://example.net/a?b=1", "noopener noreferrer"],
      ],
    },
  ])("wraps $name in anchors", ({ html, expected }) => {
    expect(anchorsIn(html)).toEqual(expected);
  });

  it("links every message the same way, however many were rendered before", () => {
    const html = "<p>One https://example.com/first</p><p>Two https://example.com/second</p>";
    const first = anchorsIn(html);
    expect(first.map(([href]) => href)).toEqual(["https://example.com/first", "https://example.com/second"]);
    expect(anchorsIn(html)).toEqual(first);
    // The plain-text path shares the pattern and must not inherit state
    // left behind by the HTML path.
    const { container } = render(<>{linkifyText("Plain https://example.com/plain")}</>);
    expect(container.querySelector("a")?.getAttribute("href")).toBe("https://example.com/plain");
  });

  it("leaves existing anchors alone and never links a script URL or breaks out of the href", () => {
    expect(anchorsIn('<p><a href="https://example.com/kept">https://example.com/kept</a></p>')).toEqual([
      ["https://example.com/kept", "https://example.com/kept", "noopener noreferrer"],
    ]);
    expect(anchorsIn("<p>javascript:alert(1)</p>")).toEqual([]);
    const sanitized = sanitizeMessageHtml("<p>https://example.com/x\"onmouseover=\"alert(1)</p>");
    const anchor = new DOMParser().parseFromString(sanitized, "text/html").querySelector("a")!;
    expect(anchor.getAttribute("href")).toBe("https://example.com/x");
    expect(anchor.getAttributeNames().sort()).toEqual(["href", "rel"]);
  });
});

it("renders plain-text URLs as links that open in the OS browser instead of navigating", () => {
  render(<SafeMessage html="" text="Come see https://example.com/offer for details." />);
  const link = screen.getByRole("link", { name: "https://example.com/offer" });
  expect(link).toHaveAttribute("href", "https://example.com/offer");

  fireEvent.click(link);
  expect(openUrl).toHaveBeenCalledWith("https://example.com/offer");
});

it.each([
  {
    name: "nested RSVP control",
    html: `
      <table><tbody><tr><td>
        <!--[if mso]><a href="https://calendar.example/event?action=RESPOND&amp;rst=1"><![endif]-->
        <a href="https://calendar.example/event?action=RESPOND&amp;rst=1" target="_blank">
          <span class="button-label">Yes</span>
        </a>
        <!--[if mso]></a><![endif]-->
      </td></tr></tbody></table>
    `,
    selector: "a span",
    expected: "https://calendar.example/event?action=RESPOND&rst=1",
  },
  {
    name: "ordinary text link",
    html: '<p>Read the <a href="https://news.example/story"><strong>full story</strong></a>.</p>',
    selector: "a strong",
    expected: "https://news.example/story",
  },
])("opens a $name in the OS browser", ({ html, selector, expected }) => {
  render(<SafeMessage html={html} />);
  const frame = screen.getByTestId("message-body") as HTMLIFrameElement;
  fireEvent.load(frame);
  // jsdom does not populate an iframe from srcdoc, so mirror the loaded
  // document before exercising the listener installed by handleLoad.
  frame.contentDocument!.body.innerHTML = new DOMParser().parseFromString(frame.srcdoc, "text/html").body.innerHTML;

  const accepted = fireEvent.click(frame.contentDocument!.querySelector(selector)!);

  expect(accepted).toBe(false);
  expect(openUrl).toHaveBeenCalledWith(expected);
});

it("leaves plain text without links untouched", () => {
  render(<SafeMessage html="" text="No links in this message." />);
  expect(screen.getByTestId("message-body").querySelector("a")).toBeNull();
});

it("blocks images by default and resolves them through resolveImage once the reader asks to load them", async () => {
  const resolveImage = vi.fn(async (url: string) => `data:image/gif;base64,RESOLVED(${url})`);
  render(<SafeMessage html="<img src='https://tracker.invalid/pixel.gif'>" resolveImage={resolveImage} />);
  const frame = screen.getByTestId("message-body") as HTMLIFrameElement;
  expect(frame.srcdoc).toContain('data-blocked-src="https://tracker.invalid/pixel.gif"');
  const live = loadFrame(frame);
  expect(resolveImage).not.toHaveBeenCalled();
  expect(screen.getByText("Load images")).toBeInTheDocument();

  fireEvent.click(screen.getByText("Load images"));
  expect(screen.queryByText("Load images")).not.toBeInTheDocument();

  await waitFor(() => {
    expect(live.querySelector("img")?.getAttribute("src")).toBe("data:image/gif;base64,RESOLVED(https://tracker.invalid/pixel.gif)");
  });
  expect(resolveImage).toHaveBeenCalledWith("https://tracker.invalid/pixel.gif");
});

it("patches resolved images into the loaded frame without reloading it", async () => {
  const resolveImage = vi.fn(async (url: string) => `data:image/png;base64,RESOLVED(${url})`);
  render(
    <SafeMessage
      html="<p>Newsletter</p><img src='https://example.com/a.png'><img src='https://example.com/b.png'>"
      loadImages
      resolveImage={resolveImage}
    />,
  );
  const frame = screen.getByTestId("message-body") as HTMLIFrameElement;
  const initialDocument = frame.srcdoc;
  const live = loadFrame(frame);

  await waitFor(() => {
    expect([...live.querySelectorAll("img")].map((image) => image.getAttribute("src"))).toEqual([
      "data:image/png;base64,RESOLVED(https://example.com/a.png)",
      "data:image/png;base64,RESOLVED(https://example.com/b.png)",
    ]);
  });
  // Rewriting srcdoc would reload the frame once per image, flashing the
  // content and losing the reader's scroll position and selection.
  expect(frame.srcdoc).toBe(initialDocument);
});

it("re-applies already resolved images when the frame reloads", async () => {
  const resolveImage = vi.fn(async (url: string) => `data:image/png;base64,RESOLVED(${url})`);
  const { rerender } = render(<SafeMessage html="<img src='https://example.com/a.png'>" loadImages resolveImage={resolveImage} theme="dark" />);
  const frame = screen.getByTestId("message-body") as HTMLIFrameElement;
  await waitFor(() => expect(resolveImage).toHaveBeenCalled());

  rerender(<SafeMessage html="<img src='https://example.com/a.png'>" loadImages resolveImage={resolveImage} theme="light" />);
  const live = loadFrame(frame);

  expect(live.querySelector("img")?.getAttribute("src")).toBe("data:image/png;base64,RESOLVED(https://example.com/a.png)");
});

it("does not restart image loading when the parent passes a new resolver each render", async () => {
  let finish: (value: string) => void = () => {};
  const first = vi.fn((_url: string) => new Promise<string>((resolve) => { finish = resolve; }));
  const { rerender } = render(<SafeMessage html="<img src='https://example.com/slow.png'>" loadImages resolveImage={first} />);
  await waitFor(() => expect(first).toHaveBeenCalledTimes(1));
  const live = loadFrame(screen.getByTestId("message-body") as HTMLIFrameElement);

  const second = vi.fn(async (_url: string) => "data:image/png;base64,SECOND");
  rerender(<SafeMessage html="<img src='https://example.com/slow.png'>" loadImages resolveImage={second} />);
  await act(async () => { finish("data:image/png;base64,FIRST"); });

  await waitFor(() => expect(live.querySelector("img")?.getAttribute("src")).toBe("data:image/png;base64,FIRST"));
  expect(second).not.toHaveBeenCalled();
});

it("resolves embedded cid images automatically while remote images remain blocked", async () => {
  const resolveImage = vi.fn(async (url: string) => `data:image/png;base64,RESOLVED(${url})`);
  render(
    <SafeMessage
      html="<img src='cid:signature.logo'><img src='https://tracker.invalid/pixel.gif'>"
      imageCacheKey="message-1"
      resolveImage={resolveImage}
    />,
  );

  await waitFor(() => expect(resolveImage).toHaveBeenCalledWith("cid:signature.logo"));
  expect(resolveImage).not.toHaveBeenCalledWith("https://tracker.invalid/pixel.gif");
  const live = loadFrame(screen.getByTestId("message-body") as HTMLIFrameElement);
  const [embedded, remote] = live.querySelectorAll("img");
  await waitFor(() => expect(embedded.getAttribute("src")).toBe("data:image/png;base64,RESOLVED(cid:signature.logo)"));
  expect(remote.getAttribute("data-blocked-src")).toBe("https://tracker.invalid/pixel.gif");
  expect(remote.hasAttribute("src")).toBe(false);
});

it("loads images automatically through resolveImage when configured", async () => {
  const resolveImage = vi.fn(async (url: string) => `data:image/png;base64,RESOLVED(${url})`);
  render(<SafeMessage html="<img src='https://example.com/logo.png'>" loadImages resolveImage={resolveImage} />);

  expect(screen.queryByText("Load images")).not.toBeInTheDocument();
  const live = loadFrame(screen.getByTestId("message-body") as HTMLIFrameElement);
  await waitFor(() => {
    expect(live.querySelector("img")?.getAttribute("src")).toBe("data:image/png;base64,RESOLVED(https://example.com/logo.png)");
  });
});

describe("shared image queue", () => {
  // The global image queue and in-flight map are module-level state. Load a
  // fresh copy of the module per test so a slot or queue entry left behind
  // by one test can never change another test's concurrency arithmetic.
  let IsolatedSafeMessage: typeof SafeMessage;
  let finishes: Array<() => void>;

  beforeEach(async () => {
    vi.resetModules();
    IsolatedSafeMessage = (await import("./SafeMessage")).SafeMessage;
    finishes = [];
  });

  afterEach(async () => {
    await settleAll();
  });

  const deferredResolver = () => vi.fn((_url: string) => new Promise<string>((resolve) => {
    finishes.push(() => resolve("data:image/png;base64,x"));
  }));

  // Resolves every started request, repeatedly, until no queued request is
  // promoted into a newly started one.
  async function settleAll() {
    for (;;) {
      await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
      const batch = finishes.splice(0);
      if (batch.length === 0) return;
      await act(async () => { batch.forEach((finish) => finish()); });
    }
  }

  it("limits concurrent image resolution within one message", async () => {
    const resolveImage = deferredResolver();
    const html = Array.from(
      { length: EMAIL_IMAGE_LIMITS.maxConcurrentPerMessage + 1 },
      (_, index) => `<img src="https://example.com/concurrency-${index}.png">`,
    ).join("");

    render(<IsolatedSafeMessage html={html} loadImages resolveImage={resolveImage} />);
    await waitFor(() => expect(resolveImage).toHaveBeenCalledTimes(EMAIL_IMAGE_LIMITS.maxConcurrentPerMessage));

    finishes.shift()!();
    await waitFor(() => expect(resolveImage).toHaveBeenCalledTimes(EMAIL_IMAGE_LIMITS.maxConcurrentPerMessage + 1));
    await settleAll();
    expect(resolveImage).toHaveBeenCalledTimes(EMAIL_IMAGE_LIMITS.maxConcurrentPerMessage + 1);
  });

  it("limits concurrent image resolution across messages", async () => {
    const resolveImage = deferredResolver();

    for (let message = 0; message < 3; message += 1) {
      render(
        <IsolatedSafeMessage
          html={`<img src="https://example.com/global-${message}-a.png"><img src="https://example.com/global-${message}-b.png">`}
          loadImages
          resolveImage={resolveImage}
        />,
      );
    }
    await waitFor(() => expect(resolveImage).toHaveBeenCalledTimes(EMAIL_IMAGE_LIMITS.maxConcurrentGlobally));

    finishes.shift()!();
    await waitFor(() => expect(resolveImage).toHaveBeenCalledTimes(EMAIL_IMAGE_LIMITS.maxConcurrentGlobally + 1));
    await settleAll();
    expect(resolveImage).toHaveBeenCalledTimes(6);
  });

  it("accepts queued images up to maxPendingGlobally and rejects the next one without requesting it", async () => {
    const resolveImage = deferredResolver();
    const limit = EMAIL_IMAGE_LIMITS.maxPendingGlobally;
    const perMessage = EMAIL_IMAGE_LIMITS.maxConcurrentPerMessage;
    // Each message holds at most maxConcurrentPerMessage requests in the
    // global queue, so fill it to one below the limit across many messages.
    const fillerUrls = Array.from({ length: limit - 1 }, (_, index) => `https://example.com/pending-${index}.png`);
    for (let offset = 0; offset < fillerUrls.length; offset += perMessage) {
      const html = fillerUrls.slice(offset, offset + perMessage).map((url) => `<img src="${url}">`).join("");
      render(<IsolatedSafeMessage html={html} loadImages resolveImage={resolveImage} />);
    }
    await waitFor(() => expect(resolveImage).toHaveBeenCalledTimes(EMAIL_IMAGE_LIMITS.maxConcurrentGlobally));

    const atLimitUrl = "https://example.com/pending-at-limit.png";
    const overLimitUrl = "https://example.com/pending-over-limit.png";
    const { container: atLimit } = render(
      <IsolatedSafeMessage html={`<img src="${atLimitUrl}">`} loadImages resolveImage={resolveImage} />,
    );
    const { container: overLimit } = render(
      <IsolatedSafeMessage html={`<img src="${overLimitUrl}">`} loadImages resolveImage={resolveImage} />,
    );

    await settleAll();
    expect(resolveImage).toHaveBeenCalledTimes(limit);
    expect(resolveImage).toHaveBeenCalledWith(atLimitUrl);
    expect(resolveImage).not.toHaveBeenCalledWith(overLimitUrl);
    expect(loadFrame(atLimit.querySelector("iframe")!).querySelector("img")?.getAttribute("src")).toBe("data:image/png;base64,x");
    const overLimitImage = loadFrame(overLimit.querySelector("iframe")!).querySelector("img");
    expect(overLimitImage?.getAttribute("data-blocked-src")).toBe(overLimitUrl);
    expect(overLimitImage?.hasAttribute("src")).toBe(false);
  });

  it("caps the number of distinct resources activated by one message", async () => {
    const resolveImage = vi.fn(async (_url: string) => "data:image/png;base64,x");
    const urls = Array.from(
      { length: EMAIL_IMAGE_LIMITS.maxImagesPerMessage + 1 },
      (_, index) => `https://example.com/fanout-${index}.png`,
    );
    const html = urls.map((url) => `<img src="${url}">`).join("");

    render(<IsolatedSafeMessage html={html} loadImages resolveImage={resolveImage} />);
    await waitFor(() => expect(resolveImage).toHaveBeenCalledTimes(EMAIL_IMAGE_LIMITS.maxImagesPerMessage));
    // Every activated image resolves immediately, so once only the
    // over-cap image is still blocked, the per-message workers are done.
    const live = loadFrame(screen.getByTestId("message-body") as HTMLIFrameElement);
    await waitFor(() => expect(live.querySelectorAll("[data-blocked-src]")).toHaveLength(1));
    await settleAll();
    expect(resolveImage).toHaveBeenCalledTimes(EMAIL_IMAGE_LIMITS.maxImagesPerMessage);
    expect(resolveImage).not.toHaveBeenCalledWith(urls[EMAIL_IMAGE_LIMITS.maxImagesPerMessage]);
    expect(live.querySelector("[data-blocked-src]")?.getAttribute("data-blocked-src")).toBe(urls[EMAIL_IMAGE_LIMITS.maxImagesPerMessage]);
  });
});

describe("frame height limit", () => {
  const limit = EMAIL_CSS_LIMITS.maxFrameHeightPx;

  it.each([
    ["below", limit - 1, limit - 1],
    ["exactly at", limit, limit],
    ["above", limit + 1, limit],
  ])("sizes the frame for content %s maxFrameHeightPx", (_case, contentHeight, expectedHeight) => {
    render(<SafeMessage html="<p>Tall content</p>" />);
    const frame = screen.getByTestId("message-body") as HTMLIFrameElement;
    Object.defineProperty(frame.contentDocument!.documentElement, "scrollHeight", {
      configurable: true,
      get: () => contentHeight,
    });

    fireEvent.load(frame);

    expect(frame.style.height).toBe(`${expectedHeight}px`);
  });
});

it("accepts data URIs through the message byte budget boundary without clamping", () => {
  const limit = EMAIL_IMAGE_LIMITS.maxDataUriBytesPerMessage;
  expect(fitsMessageImageBudget(1, limit - 2)).toBe(true);
  expect(fitsMessageImageBudget(1, limit - 1)).toBe(true);
  expect(fitsMessageImageBudget(1, limit)).toBe(false);
});

it("renders each resolved image without waiting for slower images", async () => {
  const fastUrl = "https://fast.example/per-image-render.png";
  const slowUrl = "https://slow.example/per-image-background.png";
  let finishSlow!: (dataUri: string) => void;
  const slowResult = new Promise<string>((resolve) => { finishSlow = resolve; });
  const resolveImage = vi.fn((url: string) =>
    url === slowUrl ? slowResult : Promise.resolve("data:image/png;base64,FAST="));

  render(
    <SafeMessage
      html={`<img src="${fastUrl}"><div style="background-image:url(${slowUrl})">Hero</div>`}
      loadImages
      resolveImage={resolveImage}
    />,
  );

  const live = loadFrame(screen.getByTestId("message-body") as HTMLIFrameElement);
  const hero = live.querySelector<HTMLElement>("[data-email-root] div")!;
  await waitFor(() => expect(live.querySelector("img")?.getAttribute("src")).toBe("data:image/png;base64,FAST="));
  expect(hero.getAttribute("data-blocked-src")).toBe(slowUrl);
  expect(hero.style.backgroundImage).toBe("");

  finishSlow("data:image/png;base64,SLOW=");
  await waitFor(() => {
    expect(hero.style.backgroundImage).toBe('url("data:image/png;base64,SLOW=")');
    expect(hero.hasAttribute("data-blocked-src")).toBe(false);
  });
});

it("reports the resolved src when the reader clicks an image in the message body", async () => {
  const resolveImage = vi.fn(async (url: string) => `data:image/png;base64,RESOLVED(${url})`);
  const onImageClick = vi.fn();
  render(
    <SafeMessage
      html="<img src='https://example.com/photo.png'>"
      loadImages
      resolveImage={resolveImage}
      onImageClick={onImageClick}
    />,
  );
  const live = loadFrame(screen.getByTestId("message-body") as HTMLIFrameElement);
  const image = live.querySelector("img")!;
  await waitFor(() => expect(image.getAttribute("src")).toContain("data:image/png;base64,RESOLVED"));

  fireEvent.click(image);

  expect(onImageClick).toHaveBeenCalledWith("data:image/png;base64,RESOLVED(https://example.com/photo.png)");
  expect(openUrl).not.toHaveBeenCalled();
});

it("does not report a click on a still-blocked image", () => {
  const onImageClick = vi.fn();
  render(<SafeMessage html="<img src='https://example.com/photo.png'>" onImageClick={onImageClick} />);
  const frame = screen.getByTestId("message-body") as HTMLIFrameElement;
  fireEvent.load(frame);
  frame.contentDocument!.body.innerHTML = new DOMParser().parseFromString(frame.srcdoc, "text/html").body.innerHTML;

  const image = frame.contentDocument!.querySelector("img")!;
  fireEvent.click(image);

  expect(onImageClick).not.toHaveBeenCalled();
});

it("leaves an image blocked when resolveImage rejects, rather than crashing", async () => {
  const resolveImage = vi.fn(async () => {
    throw new Error("network error");
  });
  render(<SafeMessage html="<img src='https://example.com/broken.png'>" loadImages resolveImage={resolveImage} />);

  await waitFor(() => expect(resolveImage).toHaveBeenCalled());
  // Let the rejection settle before judging the result.
  await expect(resolveImage.mock.results[0].value).rejects.toThrow("network error");
  await act(async () => {});
  const srcdoc = (screen.getByTestId("message-body") as HTMLIFrameElement).srcdoc;
  expect(srcdoc).toContain('data-blocked-src="https://example.com/broken.png"');
  expect(srcdoc).not.toMatch(/\ssrc="https:\/\/example\.com\/broken\.png"/);
});

it("opens web links separately and rejects unsafe or relative navigation", () => {
  const sanitized = sanitizeMessageHtml('<a href="https://example.com">Web</a><a href="javascript:alert(1)">Bad</a><a href="/settings">Relative</a>');
  const container = document.createElement("div");
  container.innerHTML = sanitized;
  const links = container.querySelectorAll("a");
  // No target="_blank": the click handler always intercepts navigation and
  // routes it through the native opener (see the "clicking a link" test
  // below), and setting target="_blank" on an anchor inside this sandboxed
  // iframe (no allow-popups) makes WKWebView treat it as a popup request
  // that can swallow the click before that handler's preventDefault runs.
  expect(links[0].hasAttribute("target")).toBe(false);
  expect(links[0].rel).toBe("noopener noreferrer");
  expect(links[1].hasAttribute("href")).toBe(false);
  expect(links[2].hasAttribute("href")).toBe(false);
});

it("keeps cosmetic text/box formatting: border-radius, box-shadow, text-transform, letter-spacing, white-space, word-break, border-spacing", () => {
  const sanitized = sanitizeMessageHtml(`
    <a style="border-radius:24px;box-shadow:1px 2px 4px rgba(153,153,153,0.2);text-decoration:none;" href="https://example.com">Shop now</a>
    <span style="text-transform:uppercase;letter-spacing:1px;white-space:nowrap;">Label</span>
    <table style="border-spacing:0px;"><tr><td style="word-break:break-word;">Text</td></tr></table>
  `);
  expect(sanitized).toContain("border-radius: 24px");
  expect(sanitized).toContain("box-shadow: 1px 2px 4px rgba(153,153,153,0.2)");
  expect(sanitized).toContain("text-transform: uppercase");
  expect(sanitized).toContain("letter-spacing: 1px");
  expect(sanitized).toContain("white-space: nowrap");
  expect(sanitized).toContain("word-break: break-word");
  expect(sanitized).toContain("border-spacing: 0px");
});

it("rejects unsafe values for the new cosmetic properties instead of passing them through", () => {
  const sanitized = sanitizeMessageHtml(`
    <div style="box-shadow:0 0 0 9999px red inset, url(https://tracker.invalid);border-radius:expression(alert(1));">x</div>
  `);
  expect(sanitized).not.toContain("box-shadow");
  expect(sanitized).not.toContain("border-radius");
  expect(sanitized).not.toContain("url(");
});

it("keeps font-size (px, em, rem, %) and relative line-height so a sender's type hierarchy survives", () => {
  // A heading/label/amount hierarchy (1.5em heading, 12px label, 36px
  // amount) flattens to one uniform body size if font-size is dropped.
  const blockLayout = sanitizeMessageHtml(`
    <div style="font-size:1.5em;line-height:1.3em;font-weight:bold;">Your statement is ready</div>
    <span style="font-size:12px;">AMOUNT DUE</span>
    <span style="font-size:36px;">$100.00</span>
  `);
  expect(blockLayout).toContain("font-size: 1.5em");
  expect(blockLayout).toContain("line-height: 1.3em");
  expect(blockLayout).toContain("font-size: 12px");
  expect(blockLayout).toContain("font-size: 36px");

  // Same invariant in a table layout sized with % and rem units.
  const container = document.createElement("div");
  container.innerHTML = sanitizeMessageHtml(`
    <table role="presentation"><tr>
      <th style="font-size:125%;line-height:normal">Item</th>
      <td style="font-size:0.875rem;line-height:1.25rem">Example service</td>
    </tr></table>
  `);
  expect(container.querySelector("th")?.style.fontSize).toBe("125%");
  expect(container.querySelector("th")?.style.lineHeight).toBe("normal");
  expect(container.querySelector("td")?.style.fontSize).toBe("0.875rem");
  expect(container.querySelector("td")?.style.lineHeight).toBe("1.25rem");
});

it("keeps a sender's local font stack so line-height-centered buttons retain their text metrics", () => {
  // Border-built button: the cell supplies the font stack and the anchor
  // uses a large line-height plus borders to draw the button. Replacing the
  // stack with the reader's configured font changes its ascent/descent and
  // makes the label look vertically off-center.
  const sanitized = sanitizeMessageHtml(`
    <table><tr>
      <td style="font-family: Arial, Helvetica, sans-serif">
        <a href="https://example.com"
           style="line-height:60px;border-left:20px solid #282b2e;border-right:20px solid #282b2e;border-top:10px solid #282b2e;border-bottom:10px solid #282b2e;background-color:#282b2e;font-size:16px;font-weight:bold;color:#fff;border-radius:20px;text-align:center;text-decoration:none">
          Review details
        </a>
      </td>
    </tr></table>
  `);
  const container = document.createElement("div");
  container.innerHTML = sanitized;

  expect(container.querySelector("td")?.style.fontFamily).toBe("arial, helvetica, sans-serif");
  expect(container.querySelector("a")?.style.lineHeight).toBe("60px");

  // Padding-built button: no table, the stack sits on the anchor itself via
  // a class-targeted stylesheet, and padding (not borders) draws the shape.
  const paddedHtml = `
    <style>.cta { font-family: Verdana, Geneva, sans-serif; line-height: 24px; }</style>
    <div style="text-align:center"><a class="cta" href="https://example.com/details"
      style="display:inline-block;padding:12px 24px;background-color:#1a1a1a;color:#ffffff">Open</a></div>
  `;
  expect(extractSafeStyleSheet(paddedHtml)).toMatch(/\[data-email-root\] \.cta \{[^}]*font-family: verdana, geneva, sans-serif/);
  const padded = document.createElement("div");
  padded.innerHTML = sanitizeMessageHtml(paddedHtml);
  expect(padded.querySelector("a.cta")?.getAttribute("style")).toContain("padding-top: 12px");
});

it("rejects functional font-family values", () => {
  const sanitized = sanitizeMessageHtml(`
    <span style="font-family:var(--message-font)">Variable</span>
    <span style="font-family:url(https://tracker.invalid/font)">Remote</span>
  `);
  expect(sanitized).not.toContain("font-family");
});

it("rejects an unbounded or unit-less font-size", () => {
  const sanitized = sanitizeMessageHtml('<span style="font-size:99999px;">x</span><span style="font-size:12;">y</span>');
  expect(sanitized).not.toContain("font-size");
});

it("keeps a responsive image's max-width/max-height caps alongside width: 100%", () => {
  // A responsive `width: 100%` image relies on max-width/max-height caps to
  // stay at its intended size. If the caps are stripped while width: 100%
  // survives, the image grows to the full width of whatever contains it.
  // Cap on the image itself, with an HTML width attribute as a fallback:
  const imageCapped = sanitizeMessageHtml(`
    <table role="presentation" width="100%"><tr><td style="width:100%;height:93px;">
      <img style="width:100%;height:auto;max-height:93px;max-width:93px;margin:auto;display:block;"
           width="165" src="https://example.com/thumbnail.jpg" alt="">
    </td></tr></table>
  `);
  expect(imageCapped).toContain("width: 100%");
  expect(imageCapped).toContain("max-width: 93px");
  expect(imageCapped).toContain("max-height: 93px");

  // Cap on a wrapping block instead, with the image filling it:
  const container = document.createElement("div");
  container.innerHTML = sanitizeMessageHtml(`
    <div style="max-width:120px;max-height:80px;overflow:hidden">
      <img style="width:100%;height:auto;display:block" src="https://example.com/preview.png" alt="">
    </div>
  `);
  const wrapper = container.querySelector("div")!;
  expect(wrapper.style.maxWidth).toBe("120px");
  expect(wrapper.style.maxHeight).toBe("80px");
  expect(wrapper.querySelector("img")?.style.width).toBe("100%");
});
