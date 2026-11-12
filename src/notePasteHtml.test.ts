import { describe, expect, it } from "vitest";
import { cleanPastedHtml } from "./notePasteHtml";

describe("cleanPastedHtml", () => {
  it.each([
    ["a space at the end of bold text, before a link",
      '<p><b>Complimentary Parking: </b><a href="https://maps.example/x"><i>1250 Bedford Ave.</i></a></p>',
      '<p><b>Complimentary Parking:</b> <a href="https://maps.example/x"><i>1250 Bedford Ave.</i></a></p>'],
    ["a bold paragraph holding only a non-breaking space",
      "<p><b>Venue</b></p><p><b> </b></p><p><i>Parking</i></p>",
      "<p><b>Venue</b></p><p> </p><p><i>Parking</i></p>"],
    ["spaces at both edges of nested formatting in a table cell",
      "<table><tr><td>Say<span style=\"color:red\"><strong><em> hello </em></strong></span>there</td></tr></table>",
      "<table><tbody><tr><td>Say <span style=\"color:red\"><strong><em>hello</em></strong></span> there</td></tr></tbody></table>"],
    ["a non-breaking space between words",
      "<div>Pittsburgh, PA</div>",
      "<div>Pittsburgh, PA</div>"],
  ])("moves spacing out of the way of the Markdown converter: %s", (_name, html, expected) => {
    expect(cleanPastedHtml(html)).toBe(expected);
  });

  it("keeps formatting that holds an image even without text", () => {
    expect(cleanPastedHtml('<p><a href="https://example.com"> <img src="x.png" alt=""> </a></p>'))
      .toBe('<p> <a href="https://example.com"><img src="x.png" alt=""></a></p>');
  });

  it("leaves spacing in code as written", () => {
    expect(cleanPastedHtml("<pre>a  <b> b </b></pre><p><code> x </code></p>")).toBeNull();
  });

  it("leaves the editor's own copies and already clean HTML alone", () => {
    expect(cleanPastedHtml('<p data-pm-slice="1 1 []"><b>Hi </b> </p>')).toBeNull();
    expect(cleanPastedHtml('<div data-meowdown="1"><b> x</b></div>')).toBeNull();
    expect(cleanPastedHtml("<p><b>Date:</b> Tuesday</p>")).toBeNull();
  });
});
