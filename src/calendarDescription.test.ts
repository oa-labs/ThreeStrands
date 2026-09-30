import { afterEach, describe, expect, it } from "vitest";
import { calendarDescriptionText } from "./calendarDescription";

describe("calendarDescriptionText", () => {
  afterEach(() => {
    delete (window as { calendarDescriptionProbe?: boolean }).calendarDescriptionProbe;
  });

  it("turns line breaks into newlines instead of showing literal tags", () => {
    expect(
      calendarDescriptionText(
        "<br/>Phone: +1 262-735-5488, PIN: 812868058<br/>Phone: +1 406-616-2916, PIN: 270438939",
      ),
    ).toBe("Phone: +1 262-735-5488, PIN: 812868058\nPhone: +1 406-616-2916, PIN: 270438939");
  });

  it("separates block elements and list items onto their own lines", () => {
    expect(
      calendarDescriptionText(
        "<p>Agenda</p><ul>\n  <li>Roadmap</li>\n  <li>Hiring</li>\n</ul><div><b>Join</b> by phone</div>",
      ),
    ).toBe("Agenda\nRoadmap\nHiring\nJoin by phone");
  });

  it("decodes entities and keeps link text", () => {
    expect(calendarDescriptionText('R&amp;D sync &mdash; <a href="https://example.com/doc">notes</a>')).toBe(
      "R&D sync — notes",
    );
  });

  it("leaves plain-text descriptions verbatim, including angle brackets and newlines", () => {
    expect(calendarDescriptionText("Budget < $5k & timeline > Q3\n\nBring laptops")).toBe(
      "Budget < $5k & timeline > Q3\n\nBring laptops",
    );
  });

  it("drops script and style content without running it", () => {
    const text = calendarDescriptionText(
      "<style>body{background:url(https://tracker.example/bg.png)}</style>" +
        "<script>window.calendarDescriptionProbe = true</script>" +
        '<img src="https://tracker.example/pixel.png" onerror="window.calendarDescriptionProbe = true">Dial in',
    );
    expect(text).toBe("Dial in");
    expect((window as { calendarDescriptionProbe?: boolean }).calendarDescriptionProbe).toBeUndefined();
  });
});
