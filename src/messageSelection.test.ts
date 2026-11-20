import { afterEach, describe, expect, it } from "vitest";
import { snapshotMessageSelection } from "./messageSelection";

afterEach(() => { document.body.replaceChildren(); window.getSelection()?.removeAllRanges(); });

describe("the selection snapshot", () => {
  it("finds the message holding a plain-text selection among several, without it being focused", () => {
    document.body.innerHTML = '<article class="message-card-expanded" data-message-id="a"><div class="message-body-plain">Alpha text</div></article><article class="message-card-expanded" data-message-id="b"><div class="message-body-plain">Bravo text</div></article>';
    const body = document.querySelectorAll(".message-body-plain")[1]!;
    const range = document.createRange();
    range.setStart(body.firstChild!, 0);
    range.setEnd(body.firstChild!, 5);
    window.getSelection()!.addRange(range);
    expect(snapshotMessageSelection()).toEqual({ messageId: "b", text: "Bravo" });
  });

  it("reads an HTML message through its frame's own selection without changing the frame", () => {
    document.body.innerHTML = '<article class="message-card-expanded" data-message-id="a"><div class="message-body-plain">Alpha</div></article><article class="message-card-expanded" data-message-id="b"><iframe class="message-body"></iframe></article>';
    const frame = document.querySelector("iframe")!;
    frame.contentDocument!.body.textContent = "A framed passage";
    const range = frame.contentDocument!.createRange();
    range.selectNodeContents(frame.contentDocument!.body);
    frame.contentWindow!.getSelection()!.addRange(range);
    const before = frame.contentDocument!.documentElement.outerHTML;
    expect(snapshotMessageSelection()).toEqual({ messageId: "b", text: "A framed passage" });
    expect(frame.contentDocument!.documentElement.outerHTML).toBe(before);
  });

  it("is null with no selection, and ignores a selection outside any message", () => {
    document.body.innerHTML = '<p id="outside">Outside</p><article class="message-card-expanded" data-message-id="a"><div class="message-body-plain">Alpha</div></article>';
    expect(snapshotMessageSelection()).toBeNull();
    const range = document.createRange();
    range.selectNodeContents(document.getElementById("outside")!);
    window.getSelection()!.addRange(range);
    expect(snapshotMessageSelection()).toBeNull();
  });
});
