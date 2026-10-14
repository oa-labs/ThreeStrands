import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import postcss, { type Rule } from "postcss";
import { describe, expect, it } from "vitest";

const source = readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8");
const css = postcss.parse(source);
const app = readFileSync(resolve(process.cwd(), "src/App.tsx"), "utf8");

function declarations(selector: string) {
  const values = new Map<string, string>();
  css.walkRules((rule: Rule) => {
    if (rule.parent?.type !== "root" || !rule.selectors.includes(selector)) return;
    rule.walkDecls((decl) => { values.set(decl.prop, decl.value); });
  });
  return values;
}

describe("toast layout", () => {
  it("stacks app updates and accounts from other devices bottom-left without overlapping", () => {
    const stack = app.slice(app.indexOf('<div className="toast-stack-bottom-left">'));
    const end = stack.indexOf("</div>");
    expect(stack.indexOf("<AvailableAccountsNotice")).toBeGreaterThan(0);
    expect(stack.indexOf("<AvailableAccountsNotice")).toBeLessThan(end);
    expect(stack.indexOf("<UpdateNotice />")).toBeLessThan(end);

    const container = declarations(".toast-stack-bottom-left");
    expect(container.get("position")).toBe("fixed");
    expect(container.get("bottom")).toBe("24px");
    expect(container.get("left")).toBe("24px");
    expect(container.get("flex-direction")).toBe("column");
    expect(container.has("top")).toBe(false);
    expect(declarations(".toast-stack-bottom-left > .toast").get("position")).toBe("static");
  });

  it("keeps the top-right join request from stretching to the bottom edge", () => {
    // Must outrank the base .toast rule's bottom: 24px, which comes later.
    const toast = declarations(".toast.enrollment-request-toast");
    expect(toast.get("top")).toBe("24px");
    expect(toast.get("bottom")).toBe("auto");
    expect(declarations(".enrollment-request-toast").size).toBe(0);
  });
});
