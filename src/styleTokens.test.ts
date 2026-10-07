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
  it("sizes every icon by role from ICON_SIZE instead of a number", () => {
    const dir = resolve(process.cwd(), "src");
    const raw: string[] = [];
    for (const file of readdirSync(dir).filter((name) => name.endsWith(".tsx") && !name.includes(".test."))) {
      const source = readFileSync(resolve(dir, file), "utf8");
      for (const match of source.matchAll(/\bsize=\{(?!ICON_SIZE\.[a-z]+\})[^}]*\}/g)) {
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
