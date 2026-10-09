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
    const button = document.createElement("button");
    document.body.append(button);
    button.focus();
    expect(selectedMessageQuote("older")).toEqual({ messageId: "older", text: "A selected passage" });
    expect(selectedMessageQuote("other")).toBeNull();
  });

  it("ignores selections outside the targeted message, across messages, and in headers", () => {
    document.body.innerHTML = '<article class="message-card-expanded" data-message-id="older"><header>Sender</header><div class="message-body-plain">Older message</div></article><article class="message-card-expanded" data-message-id="newer"><div class="message-body-plain">Newer message</div></article>';
    const bodies = document.querySelectorAll(".message-body-plain");
    const range = document.createRange();
    range.selectNodeContents(bodies[0]);
    window.getSelection()!.addRange(range);
    expect(selectedMessageQuote("newer")).toBeNull();
    range.setEnd(bodies[1].firstChild!, 5);
    expect(selectedMessageQuote()).toBeNull();
    range.selectNodeContents(document.querySelector("header")!);
    expect(selectedMessageQuote()).toBeNull();
    range.collapse(true);
    expect(selectedMessageQuote()).toBeNull();
  });

  it("replaces a new reply's full quote and preserves the provider header and metadata", () => {
    expect(draftWithSelectedQuote(draft, "First line\nSecond line")?.body)
      .toBe("\n\nOn Tuesday, Sender wrote:\n> First line\n> Second line");
    expect(draftWithSelectedQuote(draft, "First line")?.replyId).toBe("reply");
    expect(draftWithSelectedQuote({ ...draft, revision: 1, body: "My response" }, "Selected"))
      .toBeNull();
    expect(draftWithSelectedQuote(draft, "  ")).toBeNull();
    expect(draftWithSelectedQuote({ ...draft, bodyHtml: "<p>My response</p>" }, "Selected")).toBeNull();
    expect(draftWithSelectedQuote({ ...draft, mode: "new" }, "Selected")).toBeNull();
  });

  it.each([false, true])("quotes only selected text in a forward while escaping sender markup (HTML=%s)", (html) => {
    const header = "---------- Forwarded message ----------\nFrom: Sender <sender@example.com>\nDate: Tuesday\nSubject: Subject\nTo: me@example.com\n\n";
    const forward: Draft = {
      ...draft, mode: "forward", to: "", body: html ? "" : `\n\n${header}Full original message`,
      forwardedContent: html ? { html: '<p>Full original message<img src="https://tracker.example/pixel"></p>', text: header + "Full original message" } : null,
    };
    const selected = '<script>alert(1)</script>\r\n<img src="https://tracker.example/selected"> & text';
    const quoted = draftWithSelectedQuote(forward, selected)!;
    expect(quoted.body).toBe("");
    expect(quoted.forwardedContent?.text).toBe(`${header}> <script>alert(1)</script>\n> <img src="https://tracker.example/selected"> & text`);
    const content = document.createElement("div");
    content.innerHTML = quoted.forwardedContent!.html;
    expect(content.querySelector('blockquote[type="cite"]')?.textContent).toBe(selected.replace("\r\n", ""));
    expect(content.querySelector("script, img, a, iframe")).toBeNull();
    expect(content.textContent).not.toContain("Full original message");
    expect(quoted.attachments).toEqual(forward.attachments);
    expect(draftWithSelectedQuote({ ...forward, revision: 1 }, "Selected")).toBeNull();
    expect(draftWithSelectedQuote({ ...forward, bodyHtml: "Edited forward" }, "Selected")).toBeNull();
  });
});
