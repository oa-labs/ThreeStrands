import { describe, expect, it } from "vitest";
import type { MeetingProposal, Message } from "./domain";
import { meetingBookingUrl } from "./meetingBooking";

const proposal: MeetingProposal = {
  type: "meeting", intent: "schedule", title: "Podcast interview", participants: [], location: null,
  rawTimeLanguage: "book a time here", normalizedStart: null, normalizedEnd: null, searchRangeStart: null, searchRangeEnd: null,
  durationMinutes: null, timeZone: null, confidence: 0.9,
  evidence: { sourceMessageId: "source", excerpt: "Please book a time here for the interview." },
};
const message = (bodyHtml: string, bodyText = "Please book a time here for the interview."): Message => ({
  id: "source", threadId: "thread", sender: "jennifer@example.com", recipients: [], sentAt: "2026-11-16T10:00:00Z",
  bodyHtml, bodyText, unread: false, unsubscribe: null, attachments: [],
});

describe("meetingBookingUrl", () => {
  it.each([
    ['<p><a href="https://calendly.com/jennifer/interview?month=2026-11">Book a time here</a></p>', "https://calendly.com/jennifer/interview?month=2026-11"],
    ['<p>Schedule a meeting <a href="https://calendar.example.net/select">here</a>.</p>', "https://calendar.example.net/select"],
    ['<table><tr><td><a href="https://appointments.example.org/podcast">Choose your appointment</a></td></tr></table>', "https://appointments.example.org/podcast"],
  ])("uses an explicitly labelled booking link from the source email", (html, url) => {
    expect(meetingBookingUrl(proposal, message(html))).toBe(url);
  });

  it("finds plain-text booking URLs beside a booking instruction", () => {
    expect(meetingBookingUrl(proposal, message("", "Book a time: https://example.org/meet?length=30&host=jen"))).toBe("https://example.org/meet?length=30&host=jen");
  });

  it.each([
    '<a href="https://example.org/zoom">Join video meeting</a>',
    '<a href="https://example.org">Our website</a>',
    '<a href="javascript:alert(1)">Book a time</a>',
    '<a href="data:text/html,hello">Book a time</a>',
    '<a href="https://trusted.example@evil.example/">Book a time</a>',
    '<a href="/booking">Book a time</a>',
    '<blockquote><a href="https://old.example/">Book a time</a></blockquote>',
    '<a href="https://one.example/">Book a time</a><a href="https://two.example/">Choose a time</a>',
  ])("leaves unsafe, ambiguous, unrelated, and quoted links out", (html) => {
    expect(meetingBookingUrl(proposal, message(html))).toBeNull();
  });

  it("ignores a booking instruction in a plain-text quote", () => {
    expect(meetingBookingUrl(proposal, message("", "> Book a time: https://old.example/"))).toBeNull();
  });

  it("does not borrow another message's booking link or reinterpret an announced event", () => {
    const source = message('<a href="https://example.org/book">Book a time</a>');
    expect(meetingBookingUrl(proposal, { ...source, id: "other" })).toBeNull();
    expect(meetingBookingUrl({ ...proposal, intent: "attend" }, source)).toBeNull();
    expect(meetingBookingUrl({ ...proposal, rawTimeLanguage: "next week", evidence: { ...proposal.evidence, excerpt: "Let's talk next week" } }, source)).toBeNull();
    expect(meetingBookingUrl(proposal, undefined)).toBeNull();
  });
});
