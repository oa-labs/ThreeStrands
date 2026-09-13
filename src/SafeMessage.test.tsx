import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
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

    expect(sanitized).toContain("<p>Hello</p>");
    expect(sanitized).not.toContain("style=");
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

  it("renders safe message formatting", () => {
    render(<SafeMessage html="<p>Hello <strong>friend</strong></p>" />);
    expect(screen.getByTestId("message-body")).toHaveTextContent("Hello friend");
    expect(screen.getByText("friend").tagName).toBe("STRONG");
  });
});

it("preserves safe formatting while removing CSS requests and app classes", () => {
  const sanitized = sanitizeMessageHtml('<table class="modal"><tr><td style="text-align:center;font-weight:700;padding:200px;background-image:url(https://tracker.invalid);position:fixed;color:black">Invoice</td></tr></table>');
  expect(sanitized).toContain("text-align: center");
  expect(sanitized).toContain("font-weight: 700");
  expect(sanitized).toContain("padding-top: 32px");
  expect(sanitized).not.toMatch(/tracker|position|class=|color:/);
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
  const body = screen.getByTestId("message-body");
  expect(body.querySelector("img")?.hasAttribute("src")).toBe(false);
  expect(screen.getByText("Load images")).toBeInTheDocument();

  fireEvent.click(screen.getByText("Load images"));

  expect(screen.getByTestId("message-body").querySelector("img")?.getAttribute("src")).toBe("https://tracker.invalid/pixel.gif");
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
