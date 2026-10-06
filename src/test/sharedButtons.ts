import { expect } from "vitest";

/** Workspace header controls use only the shared button classes, so the 1-4 views look alike. */
export function expectSharedButtons(container: Element) {
  const buttons = [...container.querySelectorAll("button")];
  expect(buttons.length).toBeGreaterThan(0);
  for (const button of buttons) {
    expect(button.matches(".btn, .btn-icon, .segment"), button.outerHTML).toBe(true);
  }
}

/** A form's action row uses the shared buttons and ends with its single primary action. */
export function expectPrimaryActionLast(row: Element) {
  expectSharedButtons(row);
  const buttons = [...row.querySelectorAll("button")];
  expect(buttons.filter((button) => button.classList.contains("btn-primary"))).toHaveLength(1);
  expect(buttons.at(-1)).toHaveClass("btn-primary");
}
