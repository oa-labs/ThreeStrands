import { render, screen } from "@testing-library/react";
import axe from "axe-core";
import { describe, expect, it } from "vitest";
import { App } from "./App";

describe("read and triage accessibility", () => {
  it("has no automatically detectable serious violations", async () => {
    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });
    const result = await axe.run(container, {
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
