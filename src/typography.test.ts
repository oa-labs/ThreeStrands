import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import postcss, { type Declaration, type Rule } from "postcss";
import { describe, expect, it } from "vitest";

const css = postcss.parse(readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8"));

/** Custom properties declared on the base :root rule. */
const rootTokens = new Map<string, string>();
css.walkRules(":root", (rule) => rule.walkDecls(/^--/, (declaration) => { rootTokens.set(declaration.prop, declaration.value); }));

/** Declarations outside :root token definitions, where components set type. */
function componentDeclarations(property: string) {
  const declarations: Declaration[] = [];
  css.walkDecls(property, (declaration) => {
    if ((declaration.parent as Rule).selector?.startsWith(":root")) return;
    declarations.push(declaration);
  });
  return declarations;
}

function describeDeclaration(declaration: Declaration) {
  return `${(declaration.parent as Rule).selector.replace(/\s+/g, " ")} { ${declaration.prop}: ${declaration.value}${declaration.important ? " !important" : ""} }`;
}

function offending(property: string, allowed: RegExp) {
  return componentDeclarations(property).filter((declaration) => declaration.important || !allowed.test(declaration.value)).map(describeDeclaration);
}

/** Pixel size of a type role at the default reader scale. */
function rolePx(role: string) {
  const step = /^var\((--type-[a-z-]+)\)$/.exec(rootTokens.get(role) ?? "")?.[1];
  const px = /calc\((\d+)px \* var\(--font-scale\)\)/.exec(rootTokens.get(step ?? "") ?? "")?.[1];
  return Number(px);
}

function sizeOf(selector: string) {
  let value: string | undefined;
  css.walkRules((rule) => {
    if (rule.selectors.includes(selector)) rule.walkDecls("font-size", (declaration) => { value = declaration.value; });
  });
  return /^var\((--text-[a-z-]+)\)$/.exec(value ?? "")?.[1];
}

const roles = [...rootTokens.keys()].filter((name) => name.startsWith("--text-"));

describe("type roles", () => {
  it("defines every role from one of the nine scaled size steps", () => {
    expect(roles.length).toBeGreaterThan(0);
    for (const role of roles) {
      expect(rootTokens.get(role), role).toMatch(/^var\(--type-[a-z-]+\)$/);
      expect(rolePx(role), role).toBeGreaterThan(0);
    }
  });

  it("orders the roles from page title down to micro text", () => {
    const order = ["--text-page-title", "--text-pane-title", "--text-card-title", "--text-body", "--text-secondary", "--text-meta", "--text-micro"];
    const sizes = order.map(rolePx);
    for (let index = 1; index < sizes.length; index += 1) expect(sizes[index], order[index]).toBeLessThan(sizes[index - 1]);
    expect(rolePx("--text-reading")).toBeGreaterThan(rolePx("--text-body"));
    expect(rolePx("--text-label")).toBe(rolePx("--text-meta"));
  });

  it("ranks heading levels so h1 is the largest", () => {
    const levels = ["h1", "h2", "h3", "h4"].map((selector) => sizeOf(selector));
    expect(levels).toEqual(["--text-page-title", "--text-pane-title", "--text-card-title", "--text-body"]);
  });
});

describe("component typography", () => {
  it("sizes text only by role, never by a raw value or size step", () => {
    // Avatar initials follow the circle they sit in rather than the reader's text size.
    expect(offending("font-size", /^(var\(--text-[a-z-]+\)|inherit|calc\(var\(--avatar-size\) \* [\d.]+\))$/)).toEqual([]);
    for (const declaration of componentDeclarations("font-size")) {
      const role = /^var\((--text-[a-z-]+)\)$/.exec(declaration.value)?.[1];
      if (role) expect(roles, describeDeclaration(declaration)).toContain(role);
    }
  });

  it("uses size steps only to define roles", () => {
    const stepUses: string[] = [];
    css.walkDecls((declaration) => {
      if (!declaration.value.includes("var(--type-")) return;
      if (declaration.prop.startsWith("--text-") && (declaration.parent as Rule).selector === ":root") return;
      stepUses.push(describeDeclaration(declaration));
    });
    expect(stepUses).toEqual([]);
  });

  it("uses the weight, leading, tracking, and font family tokens", () => {
    expect(offending("font-weight", /^(var\(--weight-[a-z]+\)|inherit)$/)).toEqual([]);
    expect(offending("line-height", /^(var\(--leading-[a-z]+\)|0|normal|inherit)$/)).toEqual([]);
    expect(offending("letter-spacing", /^(var\(--tracking-[a-z]+\)|normal|inherit)$/)).toEqual([]);
    expect(offending("font-family", /^(var\(--font-(family|mono)\)|inherit)$/)).toEqual([]);
    // The shorthand would hide a raw size or family from the checks above.
    expect(offending("font", /^inherit$/)).toEqual([]);
  });

  it("keeps the four weights at standard values every font can render", () => {
    expect(["regular", "medium", "semibold", "bold"].map((name) => rootTokens.get(`--weight-${name}`))).toEqual(["400", "500", "600", "700"]);
  });

  it("uppercases text only in the section and field label recipes", () => {
    const recipes = componentDeclarations("text-transform").filter((declaration) => declaration.value === "uppercase").map((declaration) => declaration.parent as Rule);
    expect(recipes).toHaveLength(2);
    const [section, field] = recipes;
    expect(section.selectors).toContain(".eyebrow");
    expect(field.selectors).toContain(".contact-editor-grid label");
    for (const recipe of recipes) {
      const value = (property: string) => recipe.nodes.find((node): node is Declaration => node.type === "decl" && node.prop === property)?.value;
      expect(value("font-size")).toBe("var(--text-label)");
      expect(value("letter-spacing")).toBe("var(--tracking-wide)");
    }
    expect(section.nodes.some((node) => node.type === "decl" && node.prop === "font-weight" && node.value === "var(--weight-semibold)")).toBe(true);
    // Field labels wrap inputs that inherit their weight.
    expect(field.nodes.some((node) => node.type === "decl" && node.prop === "font-weight" && node.value === "var(--weight-regular)")).toBe(true);
  });

  it("leaves label type to the recipes instead of per-rule overrides", () => {
    const labelSelectors = new Set(componentDeclarations("text-transform")
      .filter((declaration) => declaration.value === "uppercase")
      .flatMap((declaration) => (declaration.parent as Rule).selectors));
    const overrides: string[] = [];
    css.walkRules((rule) => {
      if (rule.nodes.some((node) => node.type === "decl" && node.prop === "text-transform" && node.value === "uppercase")) return;
      if (!rule.selectors.some((selector) => labelSelectors.has(selector))) return;
      rule.walkDecls(/^(font-size|font-weight|letter-spacing)$/, (declaration) => { overrides.push(describeDeclaration(declaration)); });
    });
    expect(overrides).toEqual([]);
  });

  it("keeps the micro size for badges, key hints, and dense calendar blocks", () => {
    const micro = componentDeclarations("font-size").filter((declaration) => declaration.value === "var(--text-micro)");
    expect(micro.length).toBeGreaterThan(0);
    for (const declaration of micro) {
      expect(describeDeclaration(declaration)).toMatch(/badge|count|kbd|calendar-schedule-event-tight|calendar-date-tile/);
    }
  });
});

describe("component source", () => {
  it("leaves type to the stylesheet instead of inline styles", () => {
    const dir = resolve(process.cwd(), "src");
    const inline: string[] = [];
    for (const file of readdirSync(dir).filter((name) => name.endsWith(".tsx") && !name.includes(".test."))) {
      const source = readFileSync(resolve(dir, file), "utf8");
      for (const match of source.matchAll(/\b(fontSize|fontWeight|lineHeight|letterSpacing)\s*:|var\(--type-/g)) {
        const line = source.slice(0, match.index).split("\n").length;
        inline.push(`${file}:${line}`);
      }
    }
    // The reader's minimum email size is the one inline exception, and it still floors at the reading role.
    expect(inline.filter((location) => !location.startsWith("SafeMessage.tsx:"))).toEqual([]);
  });
});
