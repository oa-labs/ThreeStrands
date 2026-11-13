import { renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { CalendarEventPreview, Message, MessageAttachment } from "./domain";
import { MAX_PREVIEWABLE_CALENDAR_SIZE } from "./calendarInvitations";
import { clearInvitationEventCache, useInvitationEvent } from "./useInvitationEvent";

const invite: MessageAttachment = { id: "invite", filename: "invite.ics", mimeType: "text/calendar; method=REQUEST", size: 100 };
const message = (id: string, attachments: MessageAttachment[] = [invite]) => ({ id, attachments }) as unknown as Message;
const event = (overrides: Partial<CalendarEventPreview> = {}): CalendarEventPreview => ({
  uid: "summit@example.com", title: "Mid-Year Summit", start: "2027-01-05", end: "2027-01-06", allDay: true, timeZone: null,
  location: "TBD", description: null, organizer: "Chapter", attendeeCount: 10, recurring: false, status: "CONFIRMED", ...overrides,
});

describe("useInvitationEvent", () => {
  beforeEach(clearInvitationEventCache);
  afterEach(() => { vi.restoreAllMocks(); });

  it("gives the invitation's event and the local day it is on", async () => {
    vi.spyOn(mailClient, "previewCalendarAttachment").mockResolvedValue({ events: [event()], truncated: false });
    const hook = renderHook(() => useInvitationEvent(message("m1")));
    await waitFor(() => expect(hook.result.current?.event.title).toBe("Mid-Year Summit"));
    expect(hook.result.current?.day).toEqual(new Date(2027, 0, 5));
  });

  it("parses an invitation once, even as syncs hand over new copies of its message", async () => {
    const preview = vi.spyOn(mailClient, "previewCalendarAttachment").mockResolvedValue({ events: [event()], truncated: false });
    const hook = renderHook((props: { message: Message }) => useInvitationEvent(props.message), { initialProps: { message: message("m1") } });
    await waitFor(() => expect(hook.result.current).not.toBeNull());
    const first = hook.result.current;
    hook.rerender({ message: message("m1") });
    expect(hook.result.current).toBe(first);
    hook.unmount();
    const again = renderHook(() => useInvitationEvent(message("m1")));
    await waitFor(() => expect(again.result.current).not.toBeNull());
    expect(preview).toHaveBeenCalledTimes(1);
  });

  it("has nothing for a cancelled invitation, one that fails to parse, or a message without one", async () => {
    vi.spyOn(mailClient, "previewCalendarAttachment").mockResolvedValueOnce({ events: [event({ status: "CANCELLED" })], truncated: false });
    const cancelled = renderHook(() => useInvitationEvent(message("cancelled")));
    await waitFor(() => expect(mailClient.previewCalendarAttachment).toHaveBeenCalledTimes(1));
    expect(cancelled.result.current).toBeNull();

    vi.mocked(mailClient.previewCalendarAttachment).mockRejectedValueOnce(new Error("bad ics"));
    const failed = renderHook(() => useInvitationEvent(message("failed")));
    await waitFor(() => expect(mailClient.previewCalendarAttachment).toHaveBeenCalledTimes(2));
    expect(failed.result.current).toBeNull();

    const plain = renderHook(() => useInvitationEvent(message("plain", [{ id: "a", filename: "notes.pdf", mimeType: "application/pdf", size: 10 }])));
    const huge = renderHook(() => useInvitationEvent(message("huge", [{ ...invite, size: MAX_PREVIEWABLE_CALENDAR_SIZE + 1 }])));
    const none = renderHook(() => useInvitationEvent(null));
    expect(plain.result.current).toBeNull();
    expect(huge.result.current).toBeNull();
    expect(none.result.current).toBeNull();
    expect(mailClient.previewCalendarAttachment).toHaveBeenCalledTimes(2);
  });

  it("drops the previous conversation's invitation as soon as another message is shown", async () => {
    vi.spyOn(mailClient, "previewCalendarAttachment").mockResolvedValue({ events: [event()], truncated: false });
    const hook = renderHook((props: { message: Message | null }) => useInvitationEvent(props.message), { initialProps: { message: message("m1") as Message | null } });
    await waitFor(() => expect(hook.result.current).not.toBeNull());
    vi.mocked(mailClient.previewCalendarAttachment).mockReturnValue(new Promise(() => {}));
    hook.rerender({ message: message("m2") });
    expect(hook.result.current).toBeNull();
    hook.rerender({ message: null });
    expect(hook.result.current).toBeNull();
  });
});
