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

describe("contact detail header", () => {
  it("starts near the top of the pane while retaining responsive side spacing", () => {
    expect(lastDeclaration(".contact-profile-panel", "padding")).toBe("24px clamp(24px,4vw,56px) clamp(24px,4vw,56px)");
  });
});

describe("settings section headers", () => {
  it("lets descriptions use the available width and moves actions below when space runs out", () => {
    expect(lastDeclaration(".accounts-manager-header p", "max-width")).toBeUndefined();
    expect(lastDeclaration(".accounts-manager-header > div", "flex")).toBe("1 1 420px");
    expect(lastDeclaration(".accounts-manager-header > div", "min-width")).toBe("0");
    expect(lastDeclaration(".accounts-manager-header", "flex-wrap")).toBe("wrap");
  });
});

describe("mail workspace with the calendar schedule open", () => {
  it("keeps the context panel and schedule in separate columns", () => {
    let desktopColumns: string | undefined;
    let scheduleColumn: string | undefined;
    css.walkRules(".app-shell.mail-context-open", (rule) => {
      if (rule.parent === css) rule.walkDecls("grid-template-columns", (declaration) => { desktopColumns = declaration.value; });
    });
    css.walkRules(".app-shell.mail-context-open > .calendar-sidebar", (rule) => {
      if (rule.parent === css) rule.walkDecls("grid-column", (declaration) => { scheduleColumn = declaration.value; });
    });
    expect(desktopColumns?.split(" minmax(")).toHaveLength(5);
    expect(lastDeclaration(".context-panel", "grid-column")).toBe("4");
    expect(scheduleColumn).toBe("5");
  });

  it("returns the schedule to fixed drawer positioning on narrow screens", () => {
    let position: string | undefined;
    let column: string | undefined;
    css.walkAtRules("media", (media) => {
      if (media.params !== "(max-width: 1100px)") return;
      media.walkRules((rule) => {
        if (rule.selector === ".calendar-sidebar") rule.walkDecls("position", (declaration) => { position = declaration.value; });
        if (rule.selector === ".app-shell.mail-context-open > .calendar-sidebar") {
          rule.walkDecls("grid-column", (declaration) => { column = declaration.value; });
        }
      });
    });
    expect(position).toBe("fixed");
    expect(column).toBe("auto");
  });
});

describe("conversation participant layout", () => {
  it("bounds the badges to three font-aware rows while allowing shorter lists to size naturally", () => {
    expect(lastDeclaration(".context-participants", "max-height")).toBe("calc(var(--participant-row-height) * 3 + 12px)");
    expect(lastDeclaration(".context-participants", "gap")).toBe("6px");
    expect(lastDeclaration(".context-participants", "height")).toBeUndefined();
    expect(lastDeclaration(".context-participants", "min-height")).toBeUndefined();
    expect(lastDeclaration(".context-participants", "--participant-row-height")).toBe("max(26px, calc(var(--type-xs) * 1.4 + 6px))");
    expect(lastDeclaration(".context-participants button", "height")).toBe("var(--participant-row-height)");
    expect(lastDeclaration(".context-participants", "overflow-y")).toBe("auto");
    expect(lastDeclaration(".context-participants", "overflow-x")).toBe("hidden");
    expect(lastDeclaration(".context-participants", "scrollbar-width")).toBe("thin");
    expect(lastDeclaration(".context-participants-section", "flex-shrink")).toBe("0");
    // The picker sits inside the person block, which must not shrink in the panel's flex column either.
    expect(lastDeclaration(".context-person", "flex-shrink")).toBe("0");
    expect(lastDeclaration(".context-participants button:focus-visible", "outline-offset")).toBe("-2px");
  });
});
