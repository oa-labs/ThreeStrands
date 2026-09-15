import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  SafeMessage,
  applyResolvedImages,
  collapseQuotedHistoryHtml,
  collapseQuotedHistoryText,
  extractBlockedImageUrls,
  extractSafeStyleSheet,
  linkifyText,
  sanitizeMessageHtml,
} from "./SafeMessage";
import { emailRenderingFixtures } from "./test/emailRenderingFixtures";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

afterEach(() => {
  cleanup();
  vi.mocked(openUrl).mockClear();
});

describe("SafeMessage", () => {
  it("preserves structurally distinct notification and transactional layouts", () => {
    expect(sanitizeMessageHtml(emailRenderingFixtures.notification)).toContain("<table");
    expect(sanitizeMessageHtml(emailRenderingFixtures.transactional)).toContain('class="layout"');
    const stylesheet = extractSafeStyleSheet(emailRenderingFixtures.transactional, "dark");
    expect(stylesheet).toContain("@media screen and (max-width:600px)");
    expect(stylesheet).toContain('[data-email-root][data-theme="dark"] .dark-copy');
  });

  it.each(["newsletter", "table", "flex", "darkMode", "spacer"] as const)("keeps the %s fixture structure intact", (fixture) => {
    const sanitized = sanitizeMessageHtml(emailRenderingFixtures[fixture]);
    expect(sanitized).toBeTruthy();
    if (fixture === "spacer") expect(sanitized).toContain("&nbsp;");
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
    // sender resets remain available without any Dispatch geometry override.
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
    expect(frame.srcdoc).toContain("Content-Security-Policy");
    expect(frame.srcdoc).toContain("script-src 'none'");
    expect(frame.srcdoc).toContain("img-src data:");
    // break-word, not anywhere: anywhere shrinks a box's minimum content
    // size for auto-layout, so a narrow fixed-width table cell (a numbered
    // list's index column, say) would treat even a short 2-character
    // string as breakable and split it across lines instead of letting the
    // column render slightly wider than its width hint.
    expect(frame.srcdoc).toContain("overflow-wrap: break-word");
    expect(frame.srcdoc).not.toContain("overflow-wrap: anywhere");
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

  it("collapses a common HTML reply chain behind an ellipsis until clicked", () => {
    render(<SafeMessage html={`
      <div>My current reply</div>
      <div class="gmail_quote">
        <div>On Friday, Brian wrote:</div>
        <blockquote>Earlier message</blockquote>
      </div>
    `} />);

    const frame = screen.getByTestId("message-body") as HTMLIFrameElement;
    expect(frame.srcdoc).toContain("My current reply");
    expect(frame.srcdoc).not.toContain("Earlier message");

    fireEvent.click(screen.getByRole("button", { name: "Show quoted content" }));
    expect(frame.srcdoc).toContain("Earlier message");
    expect(screen.queryByRole("button", { name: "Show quoted content" })).not.toBeInTheDocument();
  });

  it("collapses generic original-message separators while preserving the current HTML", () => {
    const collapsed = collapseQuotedHistoryHtml(`
      <p>Current answer</p>
      <div>------ Original Message ------</div>
      <div>From Brian</div>
    `);

    expect(collapsed).toContain("Current answer");
    expect(collapsed).not.toContain("Original Message");
    expect(collapsed).not.toContain("From Brian");
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

it("applyResolvedImages fills in an <img> src and a background-image once resolved, leaving unresolved ones blocked", () => {
  const sanitized = sanitizeMessageHtml(`
    <img src="https://example.com/a.png">
    <div style="background-image:url(https://example.com/b.png)">Hi</div>
    <img src="https://example.com/still-pending.png">
  `);
  const resolved = new Map([
    ["https://example.com/a.png", "data:image/png;base64,AAA="],
    ["https://example.com/b.png", "data:image/png;base64,BBB="],
  ]);
  const filled = applyResolvedImages(sanitized, resolved);
  const container = document.createElement("div");
  container.innerHTML = filled;

  expect(container.querySelectorAll("img")[0].getAttribute("src")).toBe("data:image/png;base64,AAA=");
  expect(container.querySelector("div")?.style.backgroundImage).toBe('url("data:image/png;base64,BBB=")');
  expect(container.querySelectorAll("img")[1].hasAttribute("src")).toBe(false);
  expect(container.querySelectorAll("img")[1].getAttribute("data-blocked-src")).toBe("https://example.com/still-pending.png");
});

it("restores a background-image only once resolved, with the same URL validation as <img src>", () => {
  const blocked = sanitizeMessageHtml('<div style="background-image:url(https://example.com/hero.png)">Hi</div>');
  expect(blocked).not.toMatch(/style="[^"]*url\(/);
  expect(blocked).toContain('data-blocked-src="https://example.com/hero.png"');

  const unsafe = sanitizeMessageHtml('<div style="background-image:url(javascript:alert(1))">Hi</div>');
  expect(unsafe).not.toContain("background-image");
  expect(unsafe).not.toContain("data-blocked-src");
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

it("preserves mix-blend-mode so ESP dark-mode-inversion workarounds keep canceling out", () => {
  // Customer.io/Klaviyo/HubSpot pair a black background with screen+difference
  // blend modes to defeat Gmail's automatic color inversion; both modes are a
  // no-op against black, so without mix-blend-mode the black background has
  // nothing to cancel it out and renders as an opaque block over the text.
  const sanitized = sanitizeMessageHtml(
    '<div style="background:#000;mix-blend-mode:screen"><div style="background:#000;mix-blend-mode:difference"><p>Hi</p></div></div>',
  );
  const container = document.createElement("div");
  container.innerHTML = sanitized;
  const [outer, inner] = Array.from(container.querySelectorAll("div"));
  expect(outer.style.mixBlendMode).toBe("screen");
  expect(inner.style.mixBlendMode).toBe("difference");
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
  const text = "Current answer\n\n------ Original Message ------\nFrom Brian\nEarlier message";
  expect(collapseQuotedHistoryText(text)).toBe("Current answer");
  render(<SafeMessage html="" text={text} />);

  expect(screen.getByTestId("message-body")).toHaveTextContent("Current answer");
  expect(screen.getByTestId("message-body")).not.toHaveTextContent("Earlier message");
  fireEvent.click(screen.getByRole("button", { name: "Show quoted content" }));
  expect(screen.getByTestId("message-body")).toHaveTextContent("Earlier message");
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

it("renders plain-text URLs as links that open in the OS browser instead of navigating", () => {
  render(<SafeMessage html="" text="Come see https://example.com/offer for details." />);
  const link = screen.getByRole("link", { name: "https://example.com/offer" });
  expect(link).toHaveAttribute("href", "https://example.com/offer");

  fireEvent.click(link);
  expect(openUrl).toHaveBeenCalledWith("https://example.com/offer");
});

it("leaves plain text without links untouched", () => {
  render(<SafeMessage html="" text="No links in this message." />);
  expect(screen.getByTestId("message-body").querySelector("a")).toBeNull();
});

it("blocks images by default and resolves them through resolveImage once the reader asks to load them", async () => {
  const resolveImage = vi.fn(async (url: string) => `data:image/gif;base64,RESOLVED(${url})`);
  render(<SafeMessage html="<img src='https://tracker.invalid/pixel.gif'>" resolveImage={resolveImage} />);
  const frame = () => screen.getByTestId("message-body") as HTMLIFrameElement;
  expect(frame().srcdoc).toContain('data-blocked-src="https://tracker.invalid/pixel.gif"');
  expect(resolveImage).not.toHaveBeenCalled();
  expect(screen.getByText("Load images")).toBeInTheDocument();

  fireEvent.click(screen.getByText("Load images"));
  expect(screen.queryByText("Load images")).not.toBeInTheDocument();

  await waitFor(() => {
    expect(frame().srcdoc).toContain('src="data:image/gif;base64,RESOLVED(https://tracker.invalid/pixel.gif)"');
  });
  expect(resolveImage).toHaveBeenCalledWith("https://tracker.invalid/pixel.gif");
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
  const frame = screen.getByTestId("message-body") as HTMLIFrameElement;
  expect(frame.srcdoc).toContain("data:image/png;base64,RESOLVED(cid:signature.logo)");
  expect(frame.srcdoc).toContain('data-blocked-src="https://tracker.invalid/pixel.gif"');
});

it("loads images automatically through resolveImage when configured", async () => {
  const resolveImage = vi.fn(async (url: string) => `data:image/png;base64,RESOLVED(${url})`);
  render(<SafeMessage html="<img src='https://example.com/logo.png'>" loadImages resolveImage={resolveImage} />);

  expect(screen.queryByText("Load images")).not.toBeInTheDocument();
  await waitFor(() => {
    expect((screen.getByTestId("message-body") as HTMLIFrameElement).srcdoc)
      .toContain('src="data:image/png;base64,RESOLVED(https://example.com/logo.png)"');
  });
});

it("leaves an image blocked when resolveImage rejects, rather than crashing", async () => {
  const resolveImage = vi.fn(async () => {
    throw new Error("network error");
  });
  render(<SafeMessage html="<img src='https://example.com/broken.png'>" loadImages resolveImage={resolveImage} />);

  await waitFor(() => expect(resolveImage).toHaveBeenCalled());
  expect((screen.getByTestId("message-body") as HTMLIFrameElement).srcdoc)
    .toContain('data-blocked-src="https://example.com/broken.png"');
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
    <span style="text-transform:uppercase;letter-spacing:1px;white-space:nowrap;">Amazon</span>
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

it("keeps font-size (px, em, %) and line-height in em so a sender's heading/label/price hierarchy survives", () => {
  // Mirrors a QuickBooks invoice email: a 1.5em bold heading, a 12px label,
  // and a 36px price all lost their sizing entirely when font-size wasn't
  // in the allowlist, flattening everything to one uniform body size.
  const sanitized = sanitizeMessageHtml(`
    <div style="font-size:1.5em;line-height:1.3em;font-weight:bold;">Your invoice is ready!</div>
    <span style="font-size:12px;">BALANCE DUE</span>
    <span style="font-size:36px;">$4,770.00</span>
  `);
  expect(sanitized).toContain("font-size: 1.5em");
  expect(sanitized).toContain("line-height: 1.3em");
  expect(sanitized).toContain("font-size: 12px");
  expect(sanitized).toContain("font-size: 36px");
});

it("keeps a local font stack so border-built email buttons retain the sender's text metrics", () => {
  // This is the shape used by Paylocity's CTA: the cell supplies Arial and
  // the anchor uses a large line-height plus borders to draw the button.
  // Replacing Arial with the reader's configured font changes its
  // ascent/descent and makes the label look vertically off-center.
  const sanitized = sanitizeMessageHtml(`
    <table><tr>
      <td style="font-family: Arial, Helvetica, sans-serif">
        <a href="https://example.com"
           style="line-height:60px;border-left:20px solid #282b2e;border-right:20px solid #282b2e;border-top:10px solid #282b2e;border-bottom:10px solid #282b2e;background-color:#282b2e;font-size:16px;font-weight:bold;color:#fff;border-radius:20px;text-align:center;text-decoration:none">
          View Assigned Review(s)
        </a>
      </td>
    </tr></table>
  `);
  const container = document.createElement("div");
  container.innerHTML = sanitized;

  expect(container.querySelector("td")?.style.fontFamily).toBe("arial, helvetica, sans-serif");
  expect(container.querySelector("a")?.style.lineHeight).toBe("60px");
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

it("keeps a product thumbnail capped at its intended size instead of growing to fill its container", () => {
  // Mirrors an actual Amazon Subscribe & Save template: the image itself
  // carries a responsive `width: 100%` paired with `max-width`/`max-height`
  // caps meant to keep it thumbnail-sized, plus an HTML width attribute as
  // a fallback. If max-width/max-height get stripped while width: 100%
  // survives, the image is left free to grow to the full width of
  // whatever contains it.
  const sanitized = sanitizeMessageHtml(`
    <table role="presentation" width="100%"><tr><td style="width:100%;height:93px;">
      <img style="width:100%;height:auto;max-height:93px;max-width:93px;margin:auto;display:block;"
           width="165" src="https://m.media-amazon.com/images/I/81nGPnMJHlL.jpg" alt="">
    </td></tr></table>
  `);
  expect(sanitized).toContain("max-width: 93px");
  expect(sanitized).toContain("max-height: 93px");
});
