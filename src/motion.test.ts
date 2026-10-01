import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import postcss, { type AtRule } from "postcss";
import { describe, expect, it } from "vitest";

const css = postcss.parse(readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8"));

function durationMs(value: string): number[] {
  return [...value.matchAll(/(\d*\.?\d+)(ms|s)\b/g)].map(([, amount, unit]) => Number(amount) * (unit === "s" ? 1000 : 1));
}

describe("interface motion", () => {
  it("animates entrances with compositor-only properties", () => {
    const animated = new Set<string>();
    css.walkAtRules("keyframes", (rule) => {
      rule.walkDecls((declaration) => { animated.add(declaration.prop); });
    });
    // Opacity and transform animate without layout or paint work, so an
    // entrance never competes with the reader's input for the main thread.
    expect([...animated].sort()).toEqual(["opacity", "transform"]);
  });

  it("keeps one-shot entrances short and never leaves animated values applied", () => {
    css.walkDecls("animation", (declaration) => {
      if (declaration.value.includes("infinite")) return;
      const [duration] = durationMs(declaration.value);
      expect(duration, declaration.parent?.toString()).toBeLessThanOrEqual(200);
      // `both`/`forwards` would pin the final keyframe (e.g. a transform)
      // onto the element after the entrance, changing stacking contexts.
      expect(declaration.value).not.toMatch(/\b(both|forwards)\b/);
    });
  });

  it("turns off animations and transitions when the OS asks to reduce motion", () => {
    let override: AtRule | undefined;
    css.walkAtRules("media", (rule) => {
      if (rule.params.includes("prefers-reduced-motion: reduce")) override = rule;
    });
    expect(override).toBeDefined();
    const declarations = new Map<string, string>();
    override!.walkDecls((declaration) => { declarations.set(declaration.prop, declaration.value); });
    expect(declarations.get("animation-duration")).toMatch(/!important|^0/);
    expect(declarations.get("transition-duration")).toMatch(/!important|^0/);
  });
});
