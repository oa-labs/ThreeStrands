import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import postcss from "postcss";
import { describe, expect, it } from "vitest";

const css = postcss.parse(readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8"));
const headers = [
  ".thread-header",
  ".calendar-week-header",
  ".tasks-workspace .tasks-sidebar-header",
  ".contacts-header",
];

function lastDeclaration(selector: string, property: string) {
  let value: string | undefined;
  css.walkRules((rule) => {
    if (!rule.selectors.includes(selector)) return;
    rule.walkDecls(property, (declaration) => { value = declaration.value; });
  });
  return value;
}

describe("primary workspace headers", () => {
  it("uses the same top and left inset, height, and title spacing in all four views", () => {
    const positions = headers.map((header) => ({
      padding: lastDeclaration(header, "padding"),
      topOverride: lastDeclaration(header, "padding-top"),
      leftOverride: lastDeclaration(header, "padding-left"),
      minHeight: lastDeclaration(header, "min-height"),
      alignment: lastDeclaration(header, "align-items"),
      titleMargin: lastDeclaration(`${header} h1`, "margin"),
    }));

    expect(positions[0]).toEqual({
      padding: "20px 22px 12px",
      topOverride: undefined,
      leftOverride: undefined,
      minHeight: "90px",
      alignment: "flex-start",
      titleMargin: "5px 0 0",
    });
    for (const position of positions.slice(1)) expect(position).toEqual(positions[0]);
  });
});

describe("task detail heading", () => {
  it("uses the compact heading size in both display and edit modes", () => {
    expect(lastDeclaration(".task-detail h2", "font-size")).toBe("var(--type-heading-sm)");
    expect(lastDeclaration(".task-inline-title input", "font-size")).toBe("var(--type-heading-sm)");
  });
});

describe("mail workspace with the calendar schedule open", () => {
  it("keeps the context panel and schedule in separate columns", () => {
    expect(lastDeclaration(".app-shell.mail-context-open", "grid-template-columns")?.split(" minmax(")).toHaveLength(5);
    expect(lastDeclaration(".context-panel", "grid-column")).toBe("4");
    expect(lastDeclaration(".app-shell.mail-context-open > .calendar-sidebar", "grid-column")).toBe("5");
  });
});
