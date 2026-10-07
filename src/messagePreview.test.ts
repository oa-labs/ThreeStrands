import { describe, expect, it } from "vitest";
import { MESSAGE_PREVIEW_MAX_CHARS } from "./contactContext";
import { messagePreview } from "./messagePreview";
import { buildThreadTextIndex } from "./quotedHistory";

const plain = (bodyText: string) => ({ bodyText, bodyHtml: "" });

describe("messagePreview", () => {
  it("leaves out an Outlook-style header-block quote, using the earlier messages' text", () => {
    const earlier = "I can do any time tomorrow from 2:30 to 3:30 pm Eastern.\n\nI'm pretty sure everything's set up properly, but can do this if you really think it's necessary.";
    const reply = [
      "Let's do a quick call tomorrow to verify everything.",
      "",
      "From: Sam Rivera <sam@example.com>",
      "Sent: Wednesday, October 7, 2026 3:36 PM",
      "To: Alex Kim <alex@example.org>",
      "Subject: RE: Licenses",
      "",
      earlier,
    ].join("\r\n");
    const prior = buildThreadTextIndex([earlier, reply]).before(1);
    expect(messagePreview(plain(reply), prior)).toBe("Let's do a quick call tomorrow to verify everything.");
  });

  it("leaves out an \"On … wrote:\" quote without needing the thread", () => {
    const reply = "Sounds good, see you then.\n\nOn Tue, Oct 6, 2026 at 9:00 AM Pat Lee <pat@example.net> wrote:\n> Can we meet Thursday?\n> Thanks";
    expect(messagePreview(plain(reply))).toBe("Sounds good, see you then.");
  });

  it("drops nested plain-text link annotations and image placeholders", () => {
    const signature = "Thanks, Alex Kim E: alex@example.org<mailto:alex@example.org<mailto:alex@example.org<mailto:alex@example.org>>> [Company logo]";
    expect(messagePreview(plain(signature))).toBe("Thanks, Alex Kim E: alex@example.org [Company logo]");
    const invite = "\r\n\r\n[https://meet.example.com/brand/logo.png]<https://meet.example.com/>\r\nJoin Meeting<https://meet.example.com/j/123?pwd=abc>\r\nMeeting ID: 123 [cid:image001.png@01DB]";
    expect(messagePreview(plain(invite))).toBe("Join Meeting Meeting ID: 123");
  });

  it("keeps angle-bracketed text that is not a link annotation", () => {
    expect(messagePreview(plain("Use x <y and keep <b>bold</b> literal"))).toBe("Use x <y and keep <b>bold</b> literal");
  });

  it("previews an HTML-only message from its text", () => {
    expect(messagePreview({ bodyText: "  ", bodyHtml: "<p>Hello&nbsp;<b>there</b></p><p>Second line</p>" })).toBe("Hello there Second line");
  });

  it("stops long text at the limit, and not before", () => {
    const below = "a".repeat(MESSAGE_PREVIEW_MAX_CHARS - 1);
    const exact = "b".repeat(MESSAGE_PREVIEW_MAX_CHARS);
    const above = "c".repeat(MESSAGE_PREVIEW_MAX_CHARS + 1);
    expect(messagePreview(plain(below))).toBe(below);
    expect(messagePreview(plain(exact))).toBe(exact);
    expect(messagePreview(plain(above))).toBe(`${"c".repeat(MESSAGE_PREVIEW_MAX_CHARS)}…`);
  });
});
