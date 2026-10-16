import { describe, expect, it } from "vitest";
import { messageAtReadingLine, messagePerson, READING_LINE } from "./readingMessage";

describe("messageAtReadingLine", () => {
  const reader = { top: 100, height: 1000, atBottom: false };
  const line = reader.top + reader.height * READING_LINE;

  it("picks the last message whose top is at or above the reading line", () => {
    const messages = [{ id: "a", top: -400 }, { id: "b", top: line }, { id: "c", top: line + 1 }];
    expect(messageAtReadingLine(reader, messages)).toBe("b");
  });

  it("picks the first message while none has reached the line", () => {
    expect(messageAtReadingLine(reader, [{ id: "a", top: line + 50 }, { id: "b", top: line + 400 }])).toBe("a");
  });

  it("picks the last message once the reader is scrolled to the end, however short it is", () => {
    const messages = [{ id: "a", top: 0 }, { id: "b", top: reader.top + reader.height - 20 }];
    expect(messageAtReadingLine({ ...reader, atBottom: true }, messages)).toBe("b");
  });

  it("picks nothing without messages", () => {
    expect(messageAtReadingLine(reader, [])).toBeNull();
  });
});

describe("messagePerson", () => {
  const own = new Set(["you@example.com"]);

  it("is the sender of a message someone else sent", () => {
    expect(messagePerson({ sender: "Ana <Ana@Partner.org>", recipients: ["you@example.com"] }, own)).toBe("ana@partner.org");
  });

  it("is the first other recipient of a message the user sent", () => {
    expect(messagePerson({ sender: "You <you@example.com>", recipients: ["you@example.com, Lee <lee@vendor.net>", "ana@partner.org"] }, own)).toBe("lee@vendor.net");
  });

  it("is nobody for a note the user sent only to themselves", () => {
    expect(messagePerson({ sender: "you@example.com", recipients: ["you@example.com"] }, own)).toBeNull();
  });
});
