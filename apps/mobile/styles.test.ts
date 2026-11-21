import postcss from "postcss";
import { describe, expect, it } from "vitest";
import { readStylesheet } from "../../src/test/readStylesheet";

const css = postcss.parse(readStylesheet("apps/mobile/styles.css"));

function declarations(selector: string) {
  const properties = new Map<string, string>();
  css.walkRules(selector, rule => rule.walkDecls(declaration => {
    properties.set(declaration.prop, declaration.value);
  }));
  return properties;
}

describe("Android note layout", () => {
  it("keeps the app column independent of the header and note content's intrinsic widths", () => {
    expect(declarations("#root").get("grid-template-columns")).toBe("minmax(0, 1fr)");
    expect(declarations(".note-preview").get("overflow-wrap")).toBe("anywhere");
    expect(declarations(".note-preview table").get("overflow-x")).toBe("auto");
  });

  it("lets the sticky note title shrink and wrap beside the Back button", () => {
    const header = declarations(".app-bar-note .app-bar-inner");
    expect(header.get("flex-wrap")).toBe("nowrap");
    expect(header.get("align-items")).toBe("flex-start");
    const heading = declarations(".app-bar-note h1");
    expect(heading.get("min-inline-size")).toBe("0");
    expect(heading.get("max-block-size")).toBe("max(var(--touch), 40dvh)");
    expect(heading.get("overflow-y")).toBe("auto");
    expect(heading.has("text-overflow")).toBe(false);
    expect(heading.get("white-space")).not.toBe("nowrap");
    expect(declarations("h1, h2, strong, button, span, pre").get("overflow-wrap")).toBe("anywhere");
  });
});
