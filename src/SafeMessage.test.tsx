import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SafeMessage, applyResolvedImages, extractBlockedImageUrls, sanitizeMessageHtml } from "./SafeMessage";

afterEach(cleanup);

describe("SafeMessage", () => {
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

  it("collapses empty spacer elements so they don't render as dead whitespace", () => {
    const sanitized = sanitizeMessageHtml(`
      <div style="padding:40px 0"></div>
      <p>&nbsp;</p>
      <div><div style="margin-bottom:60px"><span></span></div></div>
      <table><tr><td style="padding:200px">&nbsp;</td><td>Real content</td></tr></table>
      <p>Hello</p>
    `);

    expect(sanitized).not.toContain("padding-top: 40px");
    expect(sanitized).not.toContain("margin-bottom: 60px");
    expect(sanitized).toContain("padding-top: 0px");
    expect(sanitized).toContain("Real content");
    expect(sanitized).toContain("Hello");
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
    expect(frame.getAttribute("sandbox")).toBe("allow-same-origin");
    expect(frame.srcdoc).toContain("<p>Hello <strong>friend</strong></p>");
    expect(frame.srcdoc).toContain("Content-Security-Policy");
    expect(frame.srcdoc).toContain("script-src 'none'");
    expect(frame.srcdoc).toContain("img-src data:");
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
});

it("preserves safe formatting and the class attribute while removing CSS requests and positioning", () => {
  const sanitized = sanitizeMessageHtml('<table class="modal"><tr><td style="text-align:center;font-weight:700;padding:200px;background-image:url(https://tracker.invalid);position:fixed;color:black">Invoice</td></tr></table>');
  expect(sanitized).toContain("text-align: center");
  expect(sanitized).toContain("font-weight: 700");
  expect(sanitized).toContain("padding-top: 32px");
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
    <table bgcolor="#fff" cellpadding="8" cellspacing="999" style="width:100%">
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
  expect(table.getAttribute("cellspacing")).toBe("32"); // capped
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
  expect(links[0].target).toBe("_blank");
  expect(links[0].rel).toBe("noopener noreferrer");
  expect(links[1].hasAttribute("href")).toBe(false);
  expect(links[2].hasAttribute("href")).toBe(false);
});
