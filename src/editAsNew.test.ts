import { describe, expect, it } from "vitest";
import type { Message } from "./domain";
import { editAsNewSource, sentByAccount } from "./editAsNew";

const base: Message = {
  id: "", threadId: "t", sender: "", recipients: [], sentAt: "2026-10-01T10:00:00Z",
  bodyHtml: "", bodyText: "", unread: false, attachments: [],
};
const thread = [
  { ...base, id: "invite", sender: "Me <ME@example.com>" },
  { ...base, id: "question", sender: "Guest <guest@example.com>" },
  { ...base, id: "reminder", sender: "me@example.com" },
  { ...base, id: "thanks", sender: "Guest <guest@example.com>" },
];

describe("editAsNewSource", () => {
  it("prefers the active message when the account sent it", () => {
    expect(editAsNewSource(thread, "me@example.com", "invite")?.id).toBe("invite");
  });

  it("falls back to the latest message the account sent", () => {
    expect(editAsNewSource(thread, "me@example.com", "thanks")?.id).toBe("reminder");
    expect(editAsNewSource(thread, "me@example.com", null)?.id).toBe("reminder");
  });

  it("finds nothing in a thread the account never wrote in", () => {
    expect(editAsNewSource(thread, "other@example.com")).toBeNull();
    expect(editAsNewSource(thread, undefined)).toBeNull();
  });

  it("compares the sender address, not its display name or a lookalike domain", () => {
    expect(sentByAccount({ ...base, sender: "me@example.com <guest@example.com>" }, "me@example.com")).toBe(false);
    expect(sentByAccount({ ...base, sender: "me@example.com.evil.test" }, "me@example.com")).toBe(false);
  });
});
