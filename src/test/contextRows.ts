import { expect } from "vitest";

/**
 * Every row a context panel section lists uses the shared ContextRow layout,
 * so indentation and the date's place stay the same across sections.
 */
export function expectContextRows(section: Element) {
  const body = section.querySelector(":scope > div");
  expect(body).not.toBeNull();
  // Notes, Show more/Load older links, and errors are not rows.
  const rows = [...body!.children].filter((child) => !child.matches(".context-section-note, .context-link-button, .form-error"));
  expect(rows.length).toBeGreaterThan(0);
  for (const row of rows) {
    expect(row.matches(".context-row"), row.outerHTML).toBe(true);
    // The date, when a row has one, ends the title line.
    const date = row.querySelector(".context-row-date");
    if (date) expect(date.parentElement).toHaveClass("context-row-line");
    // A small action ends the detail line instead, so it never pushes the date in from the edge.
    if (date && row.querySelector(".btn-icon-sm")) expect(row.querySelector(".context-row-trailing .btn-icon-sm")).toBeNull();
  }
}
