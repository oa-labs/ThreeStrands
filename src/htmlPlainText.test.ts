import { afterEach, describe, expect, it } from "vitest";
import { htmlToPlainText } from "./htmlPlainText";

describe("htmlToPlainText", () => {
  afterEach(() => {
    delete (window as { htmlPlainTextProbe?: boolean }).htmlPlainTextProbe;
  });

  it("collapses source formatting whitespace in collapse mode", () => {
    expect(htmlToPlainText("<p>Agenda\n   items</p>\n<ul>\n  <li>Roadmap</li>\n</ul>", { whitespace: "collapse" }))
      .toBe("Agenda items\nRoadmap");
  });

  it("keeps spaces and indentation in preserve mode", () => {
    expect(htmlToPlainText("Hi {first_name},<br>  - one  two<br><br><br><br>Thanks &amp; regards", { whitespace: "preserve" }))
      .toBe("Hi {first_name},\n  - one  two\n\nThanks & regards");
  });

  it("maps paragraph and div blocks to lines in preserve mode", () => {
    expect(htmlToPlainText("<div>First</div><div><br></div><div>Second</div><p>Third</p>", { whitespace: "preserve" }))
      .toBe("First\n\nSecond\nThird");
  });

  it.each(["collapse", "preserve"] as const)("never runs handlers or scripts in %s mode", (whitespace) => {
    const text = htmlToPlainText(
      "<script>window.htmlPlainTextProbe = true</script>" +
        '<img src="x" onerror="window.htmlPlainTextProbe = true"><svg onload="window.htmlPlainTextProbe = true"></svg>Body',
      { whitespace },
    );
    expect(text).toBe("Body");
    expect((window as { htmlPlainTextProbe?: boolean }).htmlPlainTextProbe).toBeUndefined();
  });
});
