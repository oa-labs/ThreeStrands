import { describe, expect, it } from "vitest";
import { messageFiles, messageSource, quoteProblem, wholeMessageText } from "./addToNote";
import { MAX_ADDITION_QUOTE_CHARS, MAX_NOTE_ATTACHMENT_BYTES } from "./addToNotePolicy";
import type { Account, Message } from "./domain";

const now = new Date(2026, 10, 12);
const accounts = [{ email: "you@example.com", displayName: "You" }] as Account[];
const message = (extra: Partial<Message> = {}): Message => ({ id: "m", threadId: "t", sender: "Ada <ada@example.com>", recipients: ["you@example.com"], sentAt: new Date(2026, 9, 30).toISOString(), bodyHtml: "", bodyText: "", unread: false, attachments: [], ...extra });

describe("where an addition comes from", () => {
  it("names the sender, or your email's recipient, with a short date", () => {
    expect(messageSource(message(), accounts, now)).toEqual({ label: "Ada", dateLabel: "Oct 30" });
    expect(messageSource(message({ sender: "ada@example.com" }), accounts, now).label).toBe("ada@example.com");
    expect(messageSource(message({ sender: "You <you@example.com>", recipients: ["Grace <grace@example.com>", "x@example.com"] }), accounts, now).label).toBe("your email to Grace");
    expect(messageSource(message({ sentByAccount: "alias@example.com", sender: "alias@example.com", recipients: [] }), accounts, now).label).toBe("your email");
    expect(messageSource(message({ sentAt: new Date(2025, 9, 30).toISOString() }), accounts, now).dateLabel).toContain("2025");
  });
});

describe("the files a message offers", () => {
  const part = (id: string, size = 10) => ({ id, filename: `${id}.png`, mimeType: "image/png", size });

  it("offers every attachment part, with whether it fits a note, and counts web images instead of offering them", () => {
    const html = '<img src="cid:a"><img src="https://x.example/1.png"><img src="https://x.example/1.png"><img src="http://y.example/2.gif"><div style="background:url(https://z.example/b.png)"></div>';
    const { files, webImages } = messageFiles(message({ bodyHtml: html, attachments: [part("a"), part("big", MAX_NOTE_ATTACHMENT_BYTES + 1), part("edge", MAX_NOTE_ATTACHMENT_BYTES)] }));
    expect(files.map((file) => [file.id, file.fits])).toEqual([["a", true], ["big", false], ["edge", true]]);
    expect(webImages).toBe(3);
  });

  it("finds no web images in a message without any, or without HTML", () => {
    expect(messageFiles(message({ bodyHtml: "<p>Hi</p><img src=\"data:image/png;base64,AAAA\">" })).webImages).toBe(0);
    expect(messageFiles(message()).webImages).toBe(0);
  });
});

describe("the text of a message", () => {
  it("is its plain text with normalized line breaks, and a passage too long is refused", () => {
    expect(wholeMessageText(message({ bodyText: "\r\nHello\r\nthere\r\n\r\n" }))).toBe("Hello\nthere");
    expect(quoteProblem(null)).toBeNull();
    expect(quoteProblem("x".repeat(MAX_ADDITION_QUOTE_CHARS))).toBeNull();
    expect(quoteProblem("x".repeat(MAX_ADDITION_QUOTE_CHARS + 1))).not.toBeNull();
  });
});
