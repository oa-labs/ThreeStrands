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

/** Shells and headers converted to the spacing scale. Later phases add dialogs, forms, and rows. */
const spacedSurfaces = [
  ".sidebar",
  ".thread-header",
  ".reader-header",
  ".calendar-week-header",
  ".calendar-sidebar-header",
  ".calendar-week-side",
  ".tasks-workspace .tasks-sidebar-header",
  ".tasks-sidebar-header",
  ".goals-pane",
  ".goals-pane-header",
  ".contacts-header",
  ".contact-profile-main",
  ".contact-profile-top",
  ".contact-context-rail",
  ".context-panel",
  ".settings-nav",
  ".settings-panel",
  ".settings-page-header",
];

describe("spacing", () => {
  it("keeps the spacing scale on a 4px grid", () => {
    const steps = [...rootTokens].filter(([name]) => /^--space-\d+$/.test(name));
    expect(steps.length).toBeGreaterThan(0);
    for (const [name, value] of steps) {
      expect(value, name).toBe(`${Number(name.slice("--space-".length)) * 4}px`);
    }
  });

  it("builds the layout roles from the spacing scale", () => {
    for (const role of ["--pane-inset", "--side-pane-inset", "--pane-header-padding", "--pane-end-padding"]) {
      const value = rootTokens.get(role);
      expect(value, role).toBeDefined();
      for (const part of value!.split(/\s+/)) expect(part, role).toMatch(/^var\(--(space-\d+|pane-inset|side-pane-inset)\)$/);
    }
  });

  it("spaces shells and headers only with scale steps and layout roles", () => {
    const raw: string[] = [];
    css.walkRules((rule) => {
      if (!rule.selectors.some((selector) => spacedSurfaces.includes(selector))) return;
      rule.walkDecls(/^(padding|margin|gap)(-|$)/, (declaration) => {
        // Strip tokens, zero, and the clamp/calc wrappers that combine them; anything left is a raw length.
        const rest = declaration.value
          .replace(/var\(--[\w-]+\)/g, "")
          .replace(/\b(clamp|calc)\(/g, "(")
          .replace(/\b\d+vw\b|\*\s*-1\b|\b0\b|auto/g, "")
          .replace(/[\s(),]/g, "");
        if (rest) raw.push(describeDeclaration(declaration));
      });
    });
    expect(raw).toEqual([]);
  });

  it("covers every surface in the list, so a renamed selector cannot silently drop out", () => {
    const seen = new Set<string>();
    css.walkRules((rule) => { for (const selector of rule.selectors) seen.add(selector); });
    expect(spacedSurfaces.filter((selector) => !seen.has(selector))).toEqual([]);
  });
});
