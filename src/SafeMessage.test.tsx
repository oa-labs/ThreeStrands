import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { SafeMessage, sanitizeMessageHtml } from "./SafeMessage";

describe("SafeMessage", () => {
  it("removes active content, remote images, and inline styles", () => {
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
