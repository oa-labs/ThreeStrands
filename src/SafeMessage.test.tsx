import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SafeMessage, sanitizeMessageHtml } from "./SafeMessage";

afterEach(cleanup);

describe("SafeMessage", () => {
  it("removes active content and unsafe inline styles, and blocks remote images by default", () => {
    const sanitized = sanitizeMessageHtml(`
      <p style="background:url(https://tracker.invalid)">Hello</p>
      <img src="https://tracker.invalid/open.gif" />
      <form action="https://attacker.invalid"><input name="token" /></form>
      <script>alert("bad")</script>
    `);

    expect(sanitized).toContain("Hello");
    // A background-image is as much a network request as <img src>, so it's
    // held behind the same block/allow gate instead of a live style.
    expect(sanitized).not.toMatch(/style="[^"]*url\(/);
    expect(sanitized).toContain('data-blocked-src="https://tracker.invalid"');
    expect(sanitized).toContain("<img");
    expect(sanitized).not.toContain(' src="');
    expect(sanitized).toContain('data-blocked-src="https://tracker.invalid/open.gif"');
    expect(sanitized).not.toContain("<form");
    expect(sanitized).not.toContain("<script");
  });

  it("loads images only when explicitly allowed", () => {
    const html = '<img src="https://example.com/logo.png" />';
    expect(sanitizeMessageHtml(html)).not.toContain(' src="');
    expect(sanitizeMessageHtml(html, { allowImages: true })).toContain('src="https://example.com/logo.png"');
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
    `, { allowImages: true });
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
    `, { allowImages: true });
    const container = document.createElement("div");
    container.innerHTML = sanitized;

    expect(container.querySelector("div")?.hasAttribute("width")).toBe(false);
    expect(container.querySelector("table")?.hasAttribute("width")).toBe(false);
    expect(container.querySelector("td")?.hasAttribute("width")).toBe(false);
    expect(container.querySelector("img")?.hasAttribute("width")).toBe(false);
    expect(container.querySelector("img")?.hasAttribute("height")).toBe(false);
  });

  it("renders safe message formatting inside a sandboxed, CSP-scoped iframe", () => {
    render(<SafeMessage html="<p>Hello <strong>friend</strong></p>" />);
    const frame = screen.getByTestId("message-body") as HTMLIFrameElement;
    expect(frame.tagName).toBe("IFRAME");
    expect(frame.getAttribute("sandbox")).toBe("allow-same-origin");
    expect(frame.srcdoc).toContain("<p>Hello <strong>friend</strong></p>");
    expect(frame.srcdoc).toContain("Content-Security-Policy");
    expect(frame.srcdoc).toContain("script-src 'none'");
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

it("preserves safe formatting while removing CSS requests and app classes", () => {
  const sanitized = sanitizeMessageHtml('<table class="modal"><tr><td style="text-align:center;font-weight:700;padding:200px;background-image:url(https://tracker.invalid);position:fixed;color:black">Invoice</td></tr></table>');
  expect(sanitized).toContain("text-align: center");
  expect(sanitized).toContain("font-weight: 700");
  expect(sanitized).toContain("padding-top: 32px");
  // background-image is gated the same way as <img src> rather than stripped
  // outright, so its URL only survives as an inert blocked-src marker.
  expect(sanitized).toContain('data-blocked-src="https://tracker.invalid"');
  expect(sanitized).not.toMatch(/style="[^"]*url\(|position|class=|color:/);
});

it("restores a background-image only once images are explicitly allowed, with the same URL validation as <img src>", () => {
  const blocked = sanitizeMessageHtml('<div style="background-image:url(https://example.com/hero.png)">Hi</div>');
  expect(blocked).not.toMatch(/style="[^"]*url\(/);
  expect(blocked).toContain('data-blocked-src="https://example.com/hero.png"');

  const allowed = sanitizeMessageHtml('<div style="background-image:url(https://example.com/hero.png)">Hi</div>', { allowImages: true });
  const container = document.createElement("div");
  container.innerHTML = allowed;
  expect(container.querySelector("div")?.style.backgroundImage).toBe('url("https://example.com/hero.png")');

  const unsafe = sanitizeMessageHtml('<div style="background-image:url(javascript:alert(1))">Hi</div>', { allowImages: true });
  expect(unsafe).not.toContain("background-image");
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

it("blocks images by default and reveals them once the reader asks to load them", () => {
  render(<SafeMessage html="<img src='https://tracker.invalid/pixel.gif'>" />);
  const frame = screen.getByTestId("message-body") as HTMLIFrameElement;
  expect(frame.srcdoc).toContain('data-blocked-src="https://tracker.invalid/pixel.gif"');
  expect(screen.getByText("Load images")).toBeInTheDocument();

  fireEvent.click(screen.getByText("Load images"));

  expect((screen.getByTestId("message-body") as HTMLIFrameElement).srcdoc)
    .toContain('src="https://tracker.invalid/pixel.gif"');
  expect(screen.queryByText("Load images")).not.toBeInTheDocument();
});

it("loads images automatically when configured", () => {
  render(<SafeMessage html="<img src='https://example.com/logo.png'>" loadImages />);

  expect((screen.getByTestId("message-body") as HTMLIFrameElement).srcdoc)
    .toContain('src="https://example.com/logo.png"');
  expect(screen.queryByText("Load images")).not.toBeInTheDocument();
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
