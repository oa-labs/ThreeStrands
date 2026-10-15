import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import postcss, { type Declaration, type Rule } from "postcss";
import { describe, expect, it } from "vitest";

const css = postcss.parse(readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8"));

const rootTokens = new Map<string, string>();
css.walkRules(":root", (rule) => rule.walkDecls(/^--/, (declaration) => { rootTokens.set(declaration.prop, declaration.value); }));

function describeDeclaration(declaration: Declaration) {
  return `${(declaration.parent as Rule).selector.replace(/\s+/g, " ")} { ${declaration.prop}: ${declaration.value} }`;
}

describe("custom properties", () => {
  it("defines every custom property the stylesheet reads", () => {
    const defined = new Set<string>();
    css.walkDecls(/^--/, (declaration) => { defined.add(declaration.prop); });
    // Layout widths, calendar colors, and similar per-element values are set from component inline styles.
    const dir = resolve(process.cwd(), "src");
    const components = readdirSync(dir)
      .filter((name) => /\.tsx?$/.test(name) && !name.includes(".test."))
      .map((name) => readFileSync(resolve(dir, name), "utf8"))
      .join("\n");
    const undefinedReads: string[] = [];
    css.walkDecls((declaration) => {
      for (const [, name] of declaration.value.matchAll(/var\((--[\w-]+)/g)) {
        if (!defined.has(name) && !components.includes(`"${name}"`)) undefinedReads.push(`${name} in ${describeDeclaration(declaration)}`);
      }
    });
    expect(undefinedReads).toEqual([]);
  });
});

/** Pixel value of a spacing token at the default scale, following var() references. */
function spacePx(name: string): number {
  const value = rootTokens.get(name) ?? "";
  const reference = /^var\((--[\w-]+)\)$/.exec(value)?.[1];
  return reference ? spacePx(reference) : Number.parseFloat(value);
}

describe("spacing", () => {
  it("keeps the spacing scale on a 4px grid, with half steps only below 12px", () => {
    const steps = [...rootTokens].filter(([name]) => /^--space-\d+(-5)?$/.test(name));
    expect(steps.length).toBeGreaterThan(0);
    for (const [name, value] of steps) {
      const step = Number(name.slice("--space-".length).replace("-", "."));
      expect(value, name).toBe(`${step * 4}px`);
      if (!Number.isInteger(step)) expect(step, name).toBeLessThan(3);
    }
  });

  it("builds the layout roles from the spacing scale", () => {
    const roles = ["--pane-inset", "--side-pane-inset", "--pane-header-padding", "--pane-end-padding", "--dialog-inset", "--list-gutter", "--list-row-inset", "--settings-label-gap"];
    for (const role of roles) {
      const value = rootTokens.get(role);
      expect(value, role).toBeDefined();
      for (const part of value!.split(/\s+/)) expect(part, role).toMatch(/^var\(--(space-\d+(-5)?|pane-inset|side-pane-inset)\)$/);
    }
  });

  it("lines list rows up with the pane title: gutter plus row inset equals the pane inset", () => {
    expect(spacePx("--list-gutter") + spacePx("--list-row-inset")).toBe(spacePx("--pane-inset"));
  });

  it("insets the Files loading and empty states from the list above them", () => {
    let padding: string | undefined;
    css.walkRules(".files-main > .context-status", (rule) => rule.walkDecls("padding", (declaration) => { padding = declaration.value; }));
    expect(padding).toBeDefined();
    expect(spacePx(/^var\((--[\w-]+)\)/.exec(padding!)?.[1] ?? "")).toBeGreaterThanOrEqual(8);
  });

  it("spaces every rule only with scale steps and layout roles", () => {
    const raw: string[] = [];
    css.walkRules((rule) => {
      if (rule.selector.startsWith(":root")) return;
      rule.walkDecls(/^(padding|margin|gap|row-gap|column-gap)(-|$)/, (declaration) => {
        // Strip tokens, zero, and the clamp/calc wrappers that combine them; anything left is a raw length.
        // Relative units (em, %, vw, vh) scale with their content or the window rather than the grid.
        const rest = declaration.value
          .replace(/var\(--[\w-]+\)/g, "")
          .replace(/\b(clamp|calc)\(/g, "(")
          .replace(/(\d*\.)?\d+(em|%|vw|vh)(?![\w-])|\*\s*-1\b|\b0\b|auto/g, "")
          .replace(/[\s(),+]/g, "");
        if (rest) raw.push(describeDeclaration(declaration));
      });
    });
    expect(raw).toEqual([]);
  });
});

describe("form controls", () => {
  it("gives every IMAP text control the same explicit height and border box", () => {
    const selectors = [
      ".imap-setup input", ".imap-setup select",
      '.imap-mailbox-mapping input:not([type="checkbox"])', ".imap-mailbox-mapping select",
    ];
    for (const selector of selectors) {
      const values = new Map<string, string>();
      css.walkRules((rule) => {
        if (rule.selectors.includes(selector)) rule.walkDecls((declaration) => { values.set(declaration.prop, declaration.value); });
      });
      expect(values.get("height"), selector).toBe("var(--control-h)");
      expect(values.get("box-sizing"), selector).toBe("border-box");
      expect(values.get("min-width"), selector).toBe("0");
      expect(values.get("background-color"), selector).toBe("var(--input)");
      expect(values.get("color"), selector).toBe("var(--text)");
    }
  });

  it("insets a dialog file picker's button so it sits centered in a control-height field", () => {
    const declarations = (selector: string) => {
      const values = new Map<string, string>();
      css.walkRules((rule) => {
        if (rule.selectors.includes(selector)) rule.walkDecls((declaration) => { values.set(declaration.prop, declaration.value); });
      });
      return values;
    };
    // The field keeps the shared text-field height; the 2px inset plus a small
    // button and the 1px border on each side add back up to --control-h.
    expect(declarations(".snippet-field input").get("height")).toBe("var(--control-h)");
    const field = declarations('.snippet-field input[type="file"]');
    expect(field.get("padding")).toBe("var(--space-0-5)");
    const button = declarations('.snippet-field input[type="file"]::file-selector-button');
    expect(button.get("height")).toBe("var(--control-h-sm)");
    expect(button.get("box-sizing")).toBe("border-box");
    expect(button.get("vertical-align")).toBe("middle");
    expect(button.get("font-size")).toBe("var(--text-control)");
  });

  it("sizes text inputs, selects, and search fields from the shared control heights", () => {
    const raw: string[] = [];
    css.walkDecls(/^(height|min-height)$/, (declaration) => {
      const rule = declaration.parent as Rule;
      const controls = rule.selectors.filter((selector) => /\b(input|select)\b|search-box|contacts-search/.test(selector)
        // Checkboxes, switches, color wells, and file pickers are not text controls.
        && !/checkbox|radio|color|file|switch|select-all|::|textarea/.test(selector));
      if (controls.length && !/^(var\(--control-h(-sm|-xs)?\)|auto)$/.test(declaration.value)) raw.push(describeDeclaration(declaration));
    });
    expect(raw).toEqual([]);
  });

  /** Whether a selector targets the native select element, not a class such as `.file-list-select`. */
  const targetsSelect = (selector: string) => /(^|[\s>+~(])select(?![\w-])/.test(selector);

  it("draws every dropdown from one shared select rule with an explicit height and a single chevron", () => {
    const base = new Map<string, string>();
    css.walkRules((rule) => {
      if (rule.selector === "select") rule.walkDecls((declaration) => { base.set(declaration.prop, declaration.value); });
    });
    expect(base.get("height")).toBe("var(--control-h)");
    expect(base.get("box-sizing")).toBe("border-box");
    expect(base.get("appearance")).toBe("none");
    expect(base.get("background-color")).toBe("var(--input)");
    expect(base.get("background-image")).toMatch(/^url\("data:image\/svg\+xml/);
    expect(base.get("font-size")).toBe("var(--text-body)");

    // A context may resize or recolor a select, but a second arrow, native
    // appearance, or a background shorthand that erases the shared chevron
    // makes dropdowns drift apart again.
    const overrides: string[] = [];
    css.walkDecls(/^(-webkit-)?appearance$|^background(-image|-position|-repeat)?$/, (declaration) => {
      const rule = declaration.parent as Rule;
      if (rule.selector === "select" || !rule.selectors?.some(targetsSelect)) return;
      overrides.push(describeDeclaration(declaration));
    });
    expect(overrides).toEqual([]);
  });

  it("gives every styled text field an explicit control height instead of relying on padding", () => {
    /** The selector without state and type filters, so `.modal-form input:not([type="checkbox"])` matches `.modal-form input`. */
    const base = (selector: string) => selector.replace(/:not\((?:[^()]|\([^()]*\))*\)/g, "").replace(/\s+/g, " ").trim();
    const isTextField = (selector: string) => /(^|[\s>+~])input(?![\w-])/.test(selector)
      && !/checkbox|radio|range|color|file|switch|::|:focus|:disabled|:hover|:checked|placeholder|data-empty/.test(selector);
    const sized = new Set<string>();
    const styled = new Map<string, string>();
    css.walkRules((rule) => {
      const decls = rule.nodes.filter((node): node is Declaration => node.type === "decl");
      for (const selector of rule.selectors ?? []) {
        if (decls.some((declaration) => /^(min-)?height$/.test(declaration.prop))) sized.add(base(selector));
        if (isTextField(selector) && decls.some((declaration) => /^(border|padding)/.test(declaration.prop))) styled.set(base(selector), rule.selector);
      }
    });
    // The command palette's oversized search bar is a deliberate display field, not a form control.
    // Recipient inputs are sized by the shared `.compose-field input` rule they also match.
    const exempt = new Set([".palette-search input", ".recipient-chip-row input"]);
    const unsized = [...styled.keys()].filter((selector) => !exempt.has(selector) && !sized.has(selector)
      // A borderless field inside a sized wrapper, like the mail search box, takes the wrapper's height.
      && !sized.has(selector.replace(/ input(\[[^\]]*\])?$/, "")));
    expect(unsized).toEqual([]);
  });

  it("sets inline dropdowns in control text and form dropdowns in body text", () => {
    const sizes = new Map<string, string>();
    css.walkDecls("font-size", (declaration) => {
      for (const selector of (declaration.parent as Rule).selectors ?? []) if (targetsSelect(selector)) sizes.set(selector, declaration.value);
    });
    for (const inline of [".compose-from select", ".contacts-bulk-bar select", ".contact-keep-in-touch > header select"]) {
      expect(sizes.get(inline), inline).toBe("var(--text-control)");
    }
    for (const [selector, value] of sizes) expect(value, selector).toMatch(/^var\(--text-(body|control)\)$/);
  });
});

/** Component declarations: everything outside the :root token definitions. */
function componentDeclarations(property: RegExp) {
  const declarations: Declaration[] = [];
  css.walkDecls(property, (declaration) => {
    const parent = declaration.parent as Rule;
    if (parent.selector?.startsWith(":root") || declaration.prop.startsWith("--")) return;
    declarations.push(declaration);
  });
  return declarations;
}

describe("alignment", () => {
  it("centers every flex row of actions, so a small button and an icon button beside it share a midline", () => {
    const flexRows = new Set<string>();
    const aligned = new Set<string>();
    css.walkRules((rule) => {
      const actions = rule.selectors.filter((selector) => /-actions$/.test(selector.trim()));
      rule.walkDecls((declaration) => {
        if (declaration.prop === "display" && /^(inline-)?flex$/.test(declaration.value)) actions.forEach((selector) => flexRows.add(selector.trim()));
        if (declaration.prop === "align-items") actions.forEach((selector) => aligned.add(selector.trim()));
      });
    });
    expect(flexRows.size).toBeGreaterThan(0);
    expect([...flexRows].filter((selector) => !aligned.has(selector))).toEqual([]);
  });
});

describe("elevation and state", () => {
  it("lifts surfaces only with the elevation tokens; rings and inset edges stay literal", () => {
    const raw = componentDeclarations(/^box-shadow$/)
      .filter((declaration) => !/^(var\(--(shadow-[a-z]+|focus-ring)\)|none|0 0 0 .+|inset .+)$/.test(declaration.value))
      .map(describeDeclaration);
    expect(raw).toEqual([]);
  });

  it("dims elements only with the state opacity tokens", () => {
    const raw = componentDeclarations(/^opacity$/)
      // Keyframes animate between fully hidden and fully shown.
      .filter((declaration) => !/^(0|1|var\(--opacity-[a-z]+\))$/.test(declaration.value))
      .map(describeDeclaration);
    expect(raw).toEqual([]);
  });

  it("times transitions only with the duration tokens", () => {
    const raw = componentDeclarations(/^transition(-duration)?$/)
      // The reduced-motion override collapses every transition, which is its own rule in motion.test.ts.
      .filter((declaration) => !declaration.important && /(\d|\.)(ms|s)\b/.test(declaration.value))
      .map(describeDeclaration);
    expect(raw).toEqual([]);
  });
});

describe("colors", () => {
  it("keeps literal colors in the theme tokens, except the accent palette preview and the all-accounts mark", () => {
    const literal = /#[0-9a-f]{3,8}\b|\brgba?\(|\bhsla?\(/i;
    const raw = componentDeclarations(/.*/)
      .filter((declaration) => literal.test(declaration.value))
      // Each swatch previews one accent regardless of the active one; the all-accounts mark is a fixed multicolor icon.
      .filter((declaration) => !/^\.accent-swatch\[data-accent=|^\.account-icon\.all-accounts$/.test((declaration.parent as Rule).selector))
      .map(describeDeclaration);
    expect(raw).toEqual([]);
  });

  it("draws text on an accent fill with --accent-contrast, so themes with a light accent stay legible", () => {
    const misses: string[] = [];
    css.walkRules((rule) => {
      if (rule.selector.startsWith(":root")) return;
      const value = (property: string) => rule.nodes.find((node): node is Declaration => node.type === "decl" && node.prop === property)?.value;
      if (value("background") === "var(--accent)" && value("color") && value("color") !== "var(--accent-contrast)") misses.push(rule.selector);
    });
    expect(misses).toEqual([]);
  });
});

describe("icon sizes", () => {
  it("draws file icons only through AttachmentIcon, so every file shows its kind", () => {
    const dir = resolve(process.cwd(), "src");
    const direct = readdirSync(dir)
      .filter((name) => name.endsWith(".tsx") && !name.includes(".test.") && name !== "AttachmentIcon.tsx")
      .filter((name) => /import \{[^}]*\bFile(Text|Image|Spreadsheet|Archive|Audio|Video|Code)?\b[^}]*\} from "lucide-react"/.test(readFileSync(resolve(dir, name), "utf8")));
    expect(direct).toEqual([]);
  });

  it("sizes every icon by role from ICON_SIZE instead of a number", () => {
    const dir = resolve(process.cwd(), "src");
    const raw: string[] = [];
    for (const file of readdirSync(dir).filter((name) => name.endsWith(".tsx") && !name.includes(".test."))) {
      const source = readFileSync(resolve(dir, file), "utf8");
      // A role constant, or a role looked up by name in a component that takes the role as a prop.
      for (const match of source.matchAll(/\bsize=\{(?!ICON_SIZE(\.[a-z]+|\[\w+\])\})[^}]*\}/g)) {
        raw.push(`${file}:${source.slice(0, match.index).split("\n").length} ${match[0]}`);
      }
    }
    expect(raw).toEqual([]);
  });
});

describe("component sizes", () => {
  it("rounds corners and stacks layers only with the radius and layer tokens", () => {
    const raw = componentDeclarations(/^(border(-[a-z]+)*-radius|z-index)$/)
      .filter((declaration) => !/^(var\(--(radius|z)-[\w-]+\)|0)$/.test(declaration.value))
      .map(describeDeclaration);
    expect(raw).toEqual([]);
  });

  it("sizes every dialog from the dialog width scale", () => {
    const raw: string[] = [];
    css.walkRules((rule) => {
      if (!rule.selectors.some((selector) => /-modal$|^\.modal$|^\.composer$/.test(selector))) return;
      rule.walkDecls("width", (declaration) => {
        // A narrow-window override may fill the window less a spacing step.
        const scaled = /^min\(var\(--dialog-w-(sm|md|lg|xl|full)\), (calc\(100vw - var\(--dialog-edge\)\)|100%)\)$/.test(declaration.value);
        const fill = /^calc\(100vw - var\(--space-\d+\)\)$/.test(declaration.value);
        if (!scaled && !fill) raw.push(describeDeclaration(declaration));
      });
    });
    expect(raw).toEqual([]);
  });

  it("gives each message attachment column room for its filename beyond the badge chrome", () => {
    // Kind icon, size, open icon, and the save and star segments take about 200px;
    // a narrower column truncates every filename down to its extension.
    let minimum = "";
    css.walkRules(".message-attachments", (rule) => rule.walkDecls("grid-template-columns", (declaration) => { minimum = declaration.value; }));
    const match = /minmax\(min\((\d+)px, 100%\), max-content\)/.exec(minimum);
    expect(match, minimum).not.toBeNull();
    expect(Number(match![1])).toBeGreaterThanOrEqual(280);
  });

  it("stretches each message attachment chip to its column so a column's chips share a right edge", () => {
    let justify = "";
    css.walkRules(".message-attachment", (rule) => rule.walkDecls("justify-self", (declaration) => { justify = declaration.value; }));
    expect(justify).toBe("stretch");
    // The open target, not the fixed segments, absorbs the stretched width.
    let grow = "";
    css.walkRules(".message-attachment > .tooltip-anchor:first-child", (rule) => rule.walkDecls("flex", (declaration) => { grow = declaration.value; }));
    expect(grow).toBe("1 1 auto");
  });

  it("lets message attachment tooltips hang outside the chip instead of clipping them", () => {
    const clipped: string[] = [];
    css.walkRules(".message-attachment", (rule) => rule.walkDecls(/^overflow/, (declaration) => { clipped.push(declaration.toString()); }));
    expect(clipped).toEqual([]);
  });

  it("gives count badges, dots, checks, and swatches one size each", () => {
    const families: [RegExp, string][] = [
      [/badge$|-view-count$/, "var(--badge-size)"],
      [/(^|\s)\.(unread|account)-dot$|now-indicator::before$/, "var(--dot-size)"],
      [/check$|checkbox$|^\.select-all input$/, "var(--check-size)"],
      [/^\.calendar-color(-default)?-swatch$/, "var(--swatch-size)"],
    ];
    const raw: string[] = [];
    css.walkRules((rule) => {
      if (rule.selector.startsWith(":root")) return;
      for (const [pattern, token] of families) {
        if (!rule.selectors.some((selector) => pattern.test(selector))) continue;
        rule.walkDecls(/^(width|height|min-width)$/, (declaration) => {
          if (/^(auto|100%|0)$/.test(declaration.value)) return;
          if (declaration.value !== token) raw.push(`${describeDeclaration(declaration)} (expected ${token})`);
        });
      }
    });
    expect(raw).toEqual([]);
  });

  it("times entrance animations with the duration and easing tokens", () => {
    const raw = componentDeclarations(/^animation$/)
      .filter((declaration) => /\d(ms|s)\b|cubic-bezier/.test(declaration.value))
      .map(describeDeclaration);
    expect(raw).toEqual([]);
  });
});
