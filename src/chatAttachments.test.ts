import { describe, expect, it } from "vitest";
import {
  MAX_ATTACHMENT_MATCHES,
  activeMention,
  applyMention,
  chatAttachmentOptions,
  isChatReadableAttachment,
  matchAttachments,
  type ChatAttachmentOption,
} from "./chatAttachments";
import type { Message, MessageAttachment } from "./domain";

const message = (id: string, attachments: MessageAttachment[]): Message => ({
  id, threadId: "t1", sender: `${id}@example.com`, recipients: [], sentAt: `2026-09-0${id.slice(1)}T00:00:00Z`,
  bodyHtml: "", bodyText: "", unread: false, attachments,
});
const file = (id: string, filename: string, mimeType = "application/octet-stream", inline = false): MessageAttachment => ({
  id, filename, mimeType, size: 10, inline,
});
const option = (filename: string): ChatAttachmentOption => ({ messageId: "m1", attachmentId: filename, filename, sender: "a", sentAt: "s" });

describe("chat attachments", () => {
  it("reads text, PDF, and Office files by extension or declared type", () => {
    for (const name of ["a.txt", "a.CSV", "a.md", "a.pdf", "a.docx", "a.xlsx", "a.pptx", "a.tsv", "a.log"]) {
      expect(isChatReadableAttachment(name, "application/octet-stream")).toBe(true);
    }
    expect(isChatReadableAttachment("download", "application/pdf")).toBe(true);
    expect(isChatReadableAttachment("download", "text/plain; charset=utf-8")).toBe(true);
    for (const [name, type] of [["a.jpg", "image/jpeg"], ["a.html", "text/html"], ["a.ics", "text/calendar"], ["a.doc", "application/msword"], ["a.xlsm", ""], ["a", ""]]) {
      expect(isChatReadableAttachment(name, type)).toBe(false);
    }
  });

  it("offers readable, non-inline attachments, newest message first", () => {
    const options = chatAttachmentOptions([
      message("m1", [file("a1", "old.pdf"), file("a2", "logo.png", "image/png")]),
      message("m2", [file("b1", "new.docx"), file("b2", "inline.txt", "text/plain", true), file("b3", "sheet.xlsx")]),
    ]);
    expect(options.map((entry) => [entry.messageId, entry.attachmentId, entry.filename])).toEqual([
      ["m2", "b1", "new.docx"], ["m2", "b3", "sheet.xlsx"], ["m1", "a1", "old.pdf"],
    ]);
    expect(options[0]).toMatchObject({ sender: "m2@example.com", sentAt: "2026-09-02T00:00:00Z" });
  });

  it("finds the @ being typed only at the start or after whitespace", () => {
    expect(activeMention("@", 1)).toEqual({ start: 0, query: "" });
    expect(activeMention("Summarize @Q3 rep", 17)).toEqual({ start: 10, query: "Q3 rep" });
    expect(activeMention("Summarize @Q3 rep", 12)).toEqual({ start: 10, query: "Q" });
    expect(activeMention("Mail sam@example.com", 20)).toBeNull();
    expect(activeMention("No mention", 10)).toBeNull();
    expect(activeMention("@file\nnext", 10)).toBeNull();
    expect(activeMention(`@${"a".repeat(80)}`, 81)).toEqual({ start: 0, query: "a".repeat(80) });
    expect(activeMention(`@${"a".repeat(81)}`, 82)).toBeNull();
  });

  it("matches filenames case-insensitively, prefix matches first, up to the list limit", () => {
    const options = [option("Final budget.xlsx"), option("Budget notes.txt"), option("deck.pptx")];
    expect(matchAttachments(options, "BUD").map((entry) => entry.filename)).toEqual(["Budget notes.txt", "Final budget.xlsx"]);
    expect(matchAttachments(options, "").map((entry) => entry.filename)).toEqual(["Final budget.xlsx", "Budget notes.txt", "deck.pptx"]);
    expect(matchAttachments(options, "zzz")).toEqual([]);
    expect(matchAttachments(options, "budget ").map((entry) => entry.filename)).toEqual(["Budget notes.txt"]);
    expect(matchAttachments(options, "deck.pptx")).toHaveLength(1);
    expect(matchAttachments(options, "Deck.pptx ")).toEqual([]);
    const many = Array.from({ length: MAX_ATTACHMENT_MATCHES + 1 }, (_, index) => option(`f${index}.txt`));
    expect(matchAttachments(many.slice(0, MAX_ATTACHMENT_MATCHES - 1), "f")).toHaveLength(MAX_ATTACHMENT_MATCHES - 1);
    expect(matchAttachments(many.slice(0, MAX_ATTACHMENT_MATCHES), "f")).toHaveLength(MAX_ATTACHMENT_MATCHES);
    expect(matchAttachments(many, "f")).toHaveLength(MAX_ATTACHMENT_MATCHES);
  });

  it("replaces the typed mention with the filename and keeps the rest of the text", () => {
    expect(applyMention("Summarize @rep", { start: 10, query: "rep" }, 14, "Q3 report.pdf"))
      .toEqual({ text: "Summarize @Q3 report.pdf ", caret: 25 });
    expect(applyMention("Compare @bu and the deck", { start: 8, query: "bu" }, 11, "budget.xlsx"))
      .toEqual({ text: "Compare @budget.xlsx and the deck", caret: 21 });
  });
});
