import { afterEach, describe, expect, it } from "vitest";
import type { Draft } from "./correspondence";
import { draftWithSelectedQuote, selectedMessageQuote } from "./selectedMessageQuote";

const draft: Draft = {
  id: "draft", revision: 0, account: "me@example.com", mode: "reply", sourceId: "message",
  threadId: "thread", replyId: "reply", references: [], to: "you@example.com", cc: "", bcc: "",
  subject: "Subject", body: "\n\nOn Tuesday, Sender wrote:\n> Full original message", bodyHtml: "",
  attachments: [], updatedAt: 0,
};

afterEach(() => {
  document.body.replaceChildren();
  window.getSelection()?.removeAllRanges();
});

describe("selected message quotes", () => {
  it("reads only text selected inside one plain message body", () => {
    document.body.innerHTML = '<article class="message-card-expanded" data-message-id="older"><div class="message-body-plain">First passage. Second passage.</div></article>';
    const body = document.querySelector(".message-body-plain")!;
    const range = document.createRange();
    range.setStart(body.firstChild!, 0);
    range.setEnd(body.firstChild!, 13);
    window.getSelection()!.addRange(range);
    expect(selectedMessageQuote()).toEqual({ messageId: "older", text: "First passage" });

    range.setEndAfter(body);
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(range);
    expect(selectedMessageQuote()).toBeNull();
  });

  it("reads the focused iframe selection with its source message id", () => {
    document.body.innerHTML = '<article class="message-card-expanded" data-message-id="older"><iframe class="message-body"></iframe></article>';
    const frame = document.querySelector("iframe")!;
    frame.contentDocument!.body.textContent = "A selected passage";
    const range = frame.contentDocument!.createRange();
    range.selectNodeContents(frame.contentDocument!.body);
    frame.contentWindow!.getSelection()!.addRange(range);
    frame.focus();
    expect(selectedMessageQuote()).toEqual({ messageId: "older", text: "A selected passage" });
  });

  it("replaces a new reply's full quote and preserves the provider header and metadata", () => {
    expect(draftWithSelectedQuote(draft, "First line\nSecond line")?.body)
      .toBe("\n\nOn Tuesday, Sender wrote:\n> First line\n> Second line");
    expect(draftWithSelectedQuote(draft, "First line")?.replyId).toBe("reply");
    expect(draftWithSelectedQuote({ ...draft, revision: 1, body: "My response" }, "Selected"))
      .toBeNull();
    expect(draftWithSelectedQuote(draft, "  ")).toBeNull();
  });
});
