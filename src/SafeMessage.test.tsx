import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { SafeMessage, sanitizeMessageHtml } from "./SafeMessage";

afterEach(cleanup);

describe("SafeMessage", () => {
  it("removes active content, remote images, and unsafe inline styles", () => {
    const sanitized = sanitizeMessageHtml(`
      <p style="background:url(https://tracker.invalid)">Hello</p>
      <img src="https://tracker.invalid/open.gif" />
      <form action="https://attacker.invalid"><input name="token" /></form>
      <script>alert("bad")</script>
    `);

    expect(sanitized).toContain("<p>Hello</p>");
    expect(sanitized).not.toContain("style");
    expect(sanitized).not.toContain("<img");
    expect(sanitized).not.toContain("<form");
    expect(sanitized).not.toContain("<script");
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
  render(<SafeMessage html="<img src='https://tracker.invalid'>" text={'Hello\n\n<script>literal text</script>'} />);
  const body = screen.getByTestId("message-body");
  expect(body.textContent).toBe('Hello\n\n<script>literal text</script>');
  expect(body.querySelector("script")).toBeNull();
  expect(body).toHaveClass("message-body-plain");
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
