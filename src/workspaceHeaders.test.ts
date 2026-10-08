import { readdirSync, readFileSync } from "node:fs";
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

/** Every <button ...> opening tag in the app's components, read with JSX braces balanced. */
function buttonTags() {
  const dir = resolve(process.cwd(), "src");
  const tags: { file: string; attributes: string }[] = [];
  for (const file of readdirSync(dir).filter((name) => name.endsWith(".tsx") && !name.includes(".test."))) {
    const source = readFileSync(resolve(dir, file), "utf8");
    for (const match of source.matchAll(/<button\b/g)) {
      let index = match.index + match[0].length;
      let depth = 0;
      for (; index < source.length; index += 1) {
        if (source[index] === "{") depth += 1;
        else if (source[index] === "}") depth -= 1;
        else if (source[index] === ">" && depth === 0) break;
      }
      tags.push({ file, attributes: source.slice(match.index + match[0].length, index) });
    }
  }
  return tags;
}

/** The raw className value of a button tag: a string literal or a brace-balanced expression. */
function classNameOf(attributes: string) {
  const start = /\bclassName=/.exec(attributes);
  if (!start) return undefined;
  const value = attributes.slice(start.index + start[0].length);
  if (value.startsWith('"')) return value.slice(0, value.indexOf('"', 1) + 1);
  let depth = 0;
  for (let index = 0; index < value.length; index += 1) {
    if (value[index] === "{") depth += 1;
    else if (value[index] === "}" && --depth === 0) return value.slice(0, index + 1);
  }
  return value;
}

/** Class names that appear literally in a button's className. */
function buttonClassNames() {
  const names = new Set<string>();
  for (const { attributes } of buttonTags()) {
    const className = classNameOf(attributes) ?? "";
    for (const literal of className.matchAll(/["`]([^"`]*)["`]/g)) {
      for (const name of literal[1].split(/[\s${}]+/)) if (/^[a-z][\w-]*$/.test(name)) names.add(name);
    }
  }
  return names;
}

function lastDeclaration(selector: string, property: string) {
  let value: string | undefined;
  css.walkRules((rule) => {
    if (!rule.selectors.includes(selector)) return;
    rule.walkDecls(property, (declaration) => { value = declaration.value; });
  });
  return value;
}

describe("primary workspace headers", () => {
  it("uses the same top, left, and right inset, height, and title spacing in all four views", () => {
    const positions = headers.map((header) => ({
      padding: lastDeclaration(header, "padding"),
      topOverride: lastDeclaration(header, "padding-top"),
      leftOverride: lastDeclaration(header, "padding-left"),
      rightOverride: lastDeclaration(header, "padding-right"),
      minHeight: lastDeclaration(header, "min-height"),
      alignment: lastDeclaration(header, "align-items"),
      titleMargin: lastDeclaration(`${header} h1`, "margin"),
      titleSize: lastDeclaration(`${header} h1`, "font-size"),
      titleWeight: lastDeclaration(`${header} h1`, "font-weight"),
      titleTracking: lastDeclaration(`${header} h1`, "letter-spacing"),
    }));

    expect(positions[0]).toEqual({
      padding: "var(--pane-header-padding)",
      topOverride: undefined,
      leftOverride: undefined,
      rightOverride: undefined,
      minHeight: "90px",
      alignment: "flex-start",
      titleMargin: "var(--space-1) 0 0",
      titleSize: "var(--text-page-title)",
      titleWeight: "var(--weight-semibold)",
      titleTracking: "var(--tracking-tight)",
    });
    for (const position of positions.slice(1)) expect(position).toEqual(positions[0]);
  });

  it("gives the reader and side-pane headers the same inset as the workspace headers", () => {
    for (const header of [".reader-header", ".calendar-sidebar-header", ".tasks-sidebar-header"]) {
      expect(lastDeclaration(header, "padding"), header).toBe("var(--pane-header-padding)");
    }
  });

  it("titles Settings with the same page title type as the four workspaces", () => {
    for (const property of ["font-size", "font-weight", "letter-spacing"]) {
      expect(lastDeclaration(".settings-page-header h2", property)).toBe(lastDeclaration(".contacts-header h1", property));
    }
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
  /** Pixel value of a length, following a var() reference to its :root token. */
  const px = (value: string | undefined): number => {
    const reference = /^var\((--[\w-]+)\)$/.exec(value ?? "")?.[1];
    return reference ? px(lastDeclaration(":root", reference)) : Number.parseFloat(value ?? "");
  };

  it("gives buttons, icon buttons, and segmented toggles one shared height", () => {
    const controlHeight = px(lastDeclaration(":root", "--control-h"));
    const segmentHeight = px(lastDeclaration(":root", "--control-h-sm"));
    expect(lastDeclaration(".btn", "height")).toBe("var(--control-h)");
    expect(lastDeclaration(".btn-icon", "height")).toBe("var(--control-h)");
    expect(lastDeclaration(".btn-icon", "width")).toBe("var(--control-h)");
    expect(lastDeclaration(".segment", "height")).toBe("var(--control-h-sm)");
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
    for (const selector of [".btn", ".btn-icon", ".segment"]) {
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
    const classes = [...buttonClassNames()];
    const accentFilled: string[] = [];
    css.walkRules((rule) => {
      const buttons = rule.selectors.filter((selector) => /(?<![\w-])button(?![\w-])/.test(selector)
        || classes.some((name) => new RegExp(`\\.${name}(?![\\w-])`).test(selector)));
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

  it("styles Settings buttons by their own class, not a section-wide button rule", () => {
    const blankets: string[] = [];
    css.walkRules((rule) => {
      blankets.push(...rule.selectors.filter((selector) => /^\.settings-section button(?![\w.[-])/.test(selector)));
    });
    expect(blankets).toEqual([]);
  });
});

describe("button system", () => {
  it("gives every button a class, so no button depends on its container for its look", () => {
    const unstyled = buttonTags()
      .filter(({ attributes }) => {
        const className = classNameOf(attributes);
        // A bare ternary with an empty branch can still leave the button classless.
        return !className || className === '""' || /\?\s*"[^"]*"\s*:\s*(""|undefined)\s*\}$/.test(className);
      })
      .map(({ file, attributes }) => `${file}: <button${attributes.slice(0, 60)}`);
    expect(unstyled).toEqual([]);
  });

  it("styles buttons by class instead of by element inside a container", () => {
    const elementRules: string[] = [];
    css.walkRules((rule) => {
      for (const selector of rule.selectors) {
        if (/(?<![\w-])button(?![\w-])/.test(selector) && !["button", "button:disabled", "button:focus-visible"].includes(selector)) {
          elementRules.push(selector);
        }
      }
    });
    expect(elementRules).toEqual([]);
  });

  it("sizes the small icon button from the shared control scale", () => {
    expect(lastDeclaration(".btn-icon-sm", "width")).toBe("var(--control-h-xs)");
    expect(lastDeclaration(".btn-icon-sm", "height")).toBe("var(--control-h-xs)");
    expect(lastDeclaration(".btn-sm", "height")).toBe("var(--control-h-sm)");
  });
});

describe("task detail heading", () => {
  it("edits the title in the detail dialog at the pane title size", () => {
    expect(lastDeclaration(".modal-form .task-detail-title-field input", "font-size")).toBe("var(--text-pane-title)");
  });
});

describe("contact detail header", () => {
  it("starts near the top of the pane while retaining responsive side spacing", () => {
    expect(lastDeclaration(".contact-profile-main", "padding")).toBe("var(--space-6) clamp(var(--space-6),4vw,var(--space-12)) clamp(var(--space-6),4vw,var(--space-12))");
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

  it("keeps the context panel on the first row even though the schedule precedes it in the DOM", () => {
    let scheduleRow: string | undefined;
    css.walkRules(".app-shell.mail-context-open > .calendar-sidebar", (rule) => {
      if (rule.parent === css) rule.walkDecls("grid-row", (declaration) => { scheduleRow = declaration.value; });
    });
    expect(scheduleRow).toBe("1");
    expect(lastDeclaration(".context-panel", "grid-row")).toBe("1");
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
    expect(lastDeclaration(".address-card", "max-width")).toBe("min(var(--popover-w), calc(100vw - 32px))");
    expect(lastDeclaration(".address-card", "min-width")).toBe("min(280px, calc(100vw - 32px))");
    expect(lastDeclaration(".address-card", "white-space")).toBe("normal");
    // Fixed at the top of the stacking order so the reader's scroll area can't clip it and other panes can't cover it.
    expect(lastDeclaration(".address-card", "position")).toBe("fixed");
    expect(lastDeclaration(".address-card", "z-index")).toBe("var(--z-popover)");
    expect(lastDeclaration(".address-contact .address-name:focus-visible", "outline")).toBe("2px solid var(--accent)");
  });
});
