import { render, screen } from "@testing-library/react";
import axe from "axe-core";
import { describe, expect, it } from "vitest";
import { App } from "./App";

describe("read and triage accessibility", () => {
  it("has no automatically detectable serious violations", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    const result = await axe.run(document.body, {
      // Message bodies render in a sandboxed iframe (see SafeMessage.tsx) whose
      // content is untrusted, sanitized email HTML axe doesn't need to police;
      // jsdom also doesn't support the cross-frame messaging axe needs to reach in.
      iframes: false,
      runOnly: {
        type: "tag",
        values: ["wcag2a", "wcag2aa", "wcag21aa"],
      },
    });
    const serious = result.violations.filter(
      (violation) => violation.impact === "serious" || violation.impact === "critical",
    );
    expect(serious).toEqual([]);
  });
});
