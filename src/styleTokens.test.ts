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

/** Shells and headers (phase 1) converted to the spacing scale. */
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

/** Dialogs, the composer, and notices (phase 2), matched by class family. Rows and lists come next. */
const spacedSurfacePatterns = [
  /modal|goal-review|goal-link|goal-delete-confirm|goal-dialog-footer|task-detail|task-editor|calendar-event-time-fields/,
  /availability-request|recovery-phrase|command-(list|item)|palette-search|shortcut-|snippet-(editor|field|option|picker)|label-(list|search|option|actions)/,
  /settings-inline-confirm|meeting-scheduler|meeting-slot/,
  /composer|compose-|recipient-|reply-assist|attachment-list|outbox-row/,
  /toast|send-notice|exit-notice|form-error/,
];

function isSpaced(selector: string) {
  return spacedSurfaces.includes(selector) || spacedSurfacePatterns.some((pattern) => pattern.test(selector));
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
    for (const role of ["--pane-inset", "--side-pane-inset", "--pane-header-padding", "--pane-end-padding", "--dialog-inset"]) {
      const value = rootTokens.get(role);
      expect(value, role).toBeDefined();
      for (const part of value!.split(/\s+/)) expect(part, role).toMatch(/^var\(--(space-\d+|pane-inset|side-pane-inset)\)$/);
    }
  });

  it("spaces converted surfaces only with scale steps and layout roles", () => {
    const raw: string[] = [];
    css.walkRules((rule) => {
      if ((rule.parent as Rule | undefined)?.selector?.startsWith(":root") || rule.selector.startsWith(":root")) return;
      if (!rule.selectors.some(isSpaced)) return;
      rule.walkDecls(/^(padding|margin|gap)(-|$)/, (declaration) => {
        // Strip tokens, zero, and the clamp/calc wrappers that combine them; anything left is a raw length.
        // Relative units (em, vw, vh) scale with their content or the window rather than the grid.
        const rest = declaration.value
          .replace(/var\(--[\w-]+\)/g, "")
          .replace(/\b(clamp|calc)\(/g, "(")
          .replace(/(\d*\.)?\d+(em|vw|vh)\b|\*\s*-1\b|\b0\b|auto/g, "")
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
    for (const pattern of spacedSurfacePatterns) expect([...seen].some((selector) => pattern.test(selector)), String(pattern)).toBe(true);
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
