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

describe("primary workspace header buttons", () => {
  const headerControls = [
    ".thread-header",
    ".filters-trigger",
    ".calendar-week-controls",
    ".tasks-sidebar-header-actions",
    ".contacts-header",
    ".contacts-view-switch",
  ];
  const shapeProperties = ["height", "min-height", "padding", "border", "border-radius", "font-size", "font-weight", "letter-spacing", "text-transform"];
  const px = (value: string | undefined) => Number.parseFloat(value ?? "");

  it("gives buttons, icon buttons, and segmented toggles one shared height", () => {
    const controlHeight = px(lastDeclaration(":root", "--control-h"));
    const segmentHeight = px(lastDeclaration(":root", "--control-h-sm"));
    expect(lastDeclaration(".btn", "height")).toBe("var(--control-h)");
    expect(lastDeclaration(".btn-icon", "height")).toBe("var(--control-h)");
    expect(lastDeclaration(".btn-icon", "width")).toBe("var(--control-h)");
    expect(lastDeclaration(".segmented > button", "height")).toBe("var(--control-h-sm)");
    const inset = px(lastDeclaration(".segmented", "padding"));
    const border = px(lastDeclaration(".segmented", "border"));
    expect(segmentHeight + 2 * inset + 2 * border).toBe(controlHeight);
  });

  it("leaves button shape to the shared classes instead of per-view rules", () => {
    const overrides: string[] = [];
    css.walkRules((rule) => {
      for (const selector of rule.selectors) {
        const scoped = headerControls.some((control) => selector.includes(control));
        const targetsButton = /\bbutton\b|\.filters-trigger(?![\w-])/.test(selector);
        if (!scoped || !targetsButton) continue;
        rule.walkDecls((declaration) => {
          if (shapeProperties.includes(declaration.prop)) overrides.push(`${selector} { ${declaration.prop} }`);
        });
      }
    });
    expect(overrides).toEqual([]);
  });

  it("keeps button labels in their written case", () => {
    for (const selector of [".btn", ".btn-icon", ".segmented > button"]) {
      expect(lastDeclaration(selector, "text-transform")).toBeUndefined();
    }
  });
});

describe("primary and form action buttons", () => {
  const actionRows = [
    ".modal-form-actions",
    ".snippet-editor-actions",
    ".task-quick-add",
    ".contact-save-bar",
    ".goal-dialog-footer",
    ".goal-review-actions",
    ".unsubscribe-actions",
  ];

  it("fills a primary button with --primary, the one primary look", () => {
    expect(lastDeclaration(".btn-primary", "background")).toBe("var(--primary)");
    expect(lastDeclaration(".btn-primary", "border-color")).toBe("var(--primary-border)");
  });

  it("never fills a button with the solid accent color", () => {
    const accentFilled: string[] = [];
    css.walkRules((rule) => {
      const buttons = rule.selectors.filter((selector) => /(?<![\w-])button(?![\w-])|\.btn(?![\w-])|\.btn-|-button(?![\w-])|-action(?![\w-])/.test(selector));
      if (!buttons.length) return;
      rule.walkDecls(/^background(-color)?$/, (declaration) => {
        if (declaration.value === "var(--accent)") accentFilled.push(buttons.join(", "));
      });
    });
    expect(accentFilled).toEqual([]);
  });

  it("leaves form action buttons to the shared classes", () => {
    const styled: string[] = [];
    css.walkRules((rule) => {
      for (const selector of rule.selectors) {
        if (actionRows.some((row) => selector.includes(row)) && /\bbutton\b/.test(selector)) styled.push(selector);
      }
    });
    expect(styled).toEqual([]);
  });

  it("keeps the Settings button rule from reshaping shared buttons", () => {
    let blanket = false;
    css.walkRules((rule) => {
      if (rule.selectors.includes(".settings-section button")) blanket = true;
    });
    expect(blanket).toBe(false);
    expect(lastDeclaration(".settings-section button:where(:not(.btn))", "padding")).toBe("7px 11px");
  });
});

describe("task detail heading", () => {
  it("edits the title in the detail dialog at the compact heading size", () => {
    expect(lastDeclaration(".modal-form .task-detail-title-field input", "font-size")).toBe("var(--type-heading-sm)");
  });
});

describe("contact detail header", () => {
  it("starts near the top of the pane while retaining responsive side spacing", () => {
    expect(lastDeclaration(".contact-profile-main", "padding")).toBe("24px clamp(24px,4vw,56px) clamp(24px,4vw,56px)");
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
  it("draws section counts as badges", () => {
    expect(lastDeclaration(".context-count", "border")).toBe("1px solid var(--border)");
    expect(lastDeclaration(".context-count", "border-radius")).toBe("var(--radius-xs)");
  });

  it("opens a reader contact card that fits narrow windows and keeps keyboard focus visible", () => {
    // The card grows with its longest line, such as the history facts, instead of wrapping it at a fixed width.
    expect(lastDeclaration(".address-card", "width")).toBe("max-content");
    expect(lastDeclaration(".address-card", "max-width")).toBe("min(440px, calc(100vw - 32px))");
    expect(lastDeclaration(".address-card", "min-width")).toBe("min(280px, calc(100vw - 32px))");
    expect(lastDeclaration(".address-card", "white-space")).toBe("normal");
    // Fixed at the top of the stacking order so the reader's scroll area can't clip it and other panes can't cover it.
    expect(lastDeclaration(".address-card", "position")).toBe("fixed");
    expect(lastDeclaration(".address-card", "z-index")).toBe("var(--z-popover)");
    expect(lastDeclaration(".address-contact .address-name:focus-visible", "outline")).toBe("2px solid var(--accent)");
  });
});
