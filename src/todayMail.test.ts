import { describe, expect, it } from "vitest";
import { groupTodayMail, todayMailGroup } from "./todayMail";
import { todayItem } from "./test/todayMail";

describe("todayMailGroup", () => {
  it("asks for a reply when someone you write to wrote to you directly", () => {
    expect(todayMailGroup(todayItem("a"))).toBe("reply");
  });

  it("files mail from people you haven't written to, or that only copies you, under From people", () => {
    expect(todayMailGroup(todayItem("new", { corresponded: false }))).toBe("people");
    expect(todayMailGroup(todayItem("cc", { addressedToMe: false }))).toBe("people");
  });

  it("files list mail under Updates whoever sends it, even from someone you write to", () => {
    expect(todayMailGroup(todayItem("list", { listUnsubscribe: true }))).toBe("updates");
    expect(todayMailGroup(todayItem("promo", {}, { labels: ["INBOX", "CATEGORY_PROMOTIONS"] }))).toBe("updates");
    expect(todayMailGroup(todayItem("notice", {}, { labels: ["INBOX", "CATEGORY_UPDATES"] }))).toBe("updates");
  });

  it("files no-reply addresses under Updates, and leaves look-alike local parts alone", () => {
    expect(todayMailGroup(todayItem("n1", { sender: "Shop <no-reply@shop.example>" }))).toBe("updates");
    expect(todayMailGroup(todayItem("n2", { sender: "DoNotReply@bank.example" }))).toBe("updates");
    expect(todayMailGroup(todayItem("n3", { sender: "noreply+alerts@service.example" }))).toBe("updates");
    expect(todayMailGroup(todayItem("n4", { sender: "Norma Replyford <noreplyford@example.org>" }))).toBe("reply");
  });

  it("keeps a conversation only you wrote in out of Updates", () => {
    expect(todayMailGroup(todayItem("self", { sender: null, addressedToMe: false }))).toBe("people");
  });
});

describe("groupTodayMail", () => {
  it("keeps the newest-first order inside each group", () => {
    const groups = groupTodayMail([
      todayItem("r1"), todayItem("u1", { listUnsubscribe: true }), todayItem("p1", { corresponded: false }), todayItem("r2"),
    ]);
    expect(groups.reply.map((item) => item.thread.id)).toEqual(["r1", "r2"]);
    expect(groups.people.map((item) => item.thread.id)).toEqual(["p1"]);
    expect(groups.updates.map((item) => item.thread.id)).toEqual(["u1"]);
  });
});
