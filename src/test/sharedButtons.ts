import { expect } from "vitest";

/** Workspace header controls use only the shared button classes, so the 1-4 views look alike. */
export function expectSharedButtons(container: Element) {
  const buttons = [...container.querySelectorAll("button")];
  expect(buttons.length).toBeGreaterThan(0);
  for (const button of buttons) {
    expect(button.matches(".btn, .btn-icon, .segmented > button"), button.outerHTML).toBe(true);
  }
}
