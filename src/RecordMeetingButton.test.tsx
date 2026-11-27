import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ScheduleEvent } from "./domain";
import { meetingEventReference } from "./meetingNoteEntry";
import { RecordMeetingButton } from "./RecordMeetingButton";
import { RecordingContext } from "./recordingContext";
import { recordingController } from "./test/recordingController";

const event: ScheduleEvent = {
  id: "evt", seriesId: "series", accountId: "me@example.com", calendarId: "work", title: "Weekly Sync",
  start: "2026-11-27T15:00:00Z", end: "2026-11-27T16:00:00Z", allDay: false,
};

describe("RecordMeetingButton", () => {
  beforeEach(() => { vi.useFakeTimers(); vi.setSystemTime(new Date("2026-11-27T15:05:00Z")); });
  afterEach(() => { cleanup(); vi.useRealTimers(); });

  it("starts a recording for the occurrence Add Note links, titled for the event", () => {
    const controller = recordingController();
    render(<RecordingContext.Provider value={controller}><RecordMeetingButton event={event} /></RecordingContext.Provider>);
    fireEvent.click(screen.getByRole("button", { name: "Record" }));
    expect(controller.start).toHaveBeenCalledWith({ eventRef: meetingEventReference(event), title: "Weekly Sync" });
    expect(meetingEventReference(event)).toMatchObject({ kind: "event", id: "evt", series: "series" });
  });

  it("forwards attendee hints when starting an event with attendees", () => {
    const controller = recordingController();
    const withAttendees: ScheduleEvent = { ...event, attendees: ["alice@example.com", "bob@example.com"] };
    render(<RecordingContext.Provider value={controller}><RecordMeetingButton event={withAttendees} /></RecordingContext.Provider>);
    fireEvent.click(screen.getByRole("button", { name: "Record" }));
    expect(controller.start).toHaveBeenCalledWith({
      eventRef: meetingEventReference(withAttendees),
      title: "Weekly Sync",
      attendees: ["alice@example.com", "bob@example.com"],
    });
  });

  it("reads Stop Recording while this occurrence is being recorded, and stops it", () => {
    const controller = recordingController({ status: { ...recordingController().status, state: "recording", eventRef: meetingEventReference(event) } });
    render(<RecordingContext.Provider value={controller}><RecordMeetingButton event={event} /></RecordingContext.Provider>);
    fireEvent.click(screen.getByRole("button", { name: "Stop Recording" }));
    expect(controller.stop).toHaveBeenCalledOnce();
    expect(controller.start).not.toHaveBeenCalled();
  });

  it("names the meeting when asked, for lists of several", () => {
    render(<RecordingContext.Provider value={recordingController()}><RecordMeetingButton event={event} labelSuffix="Weekly Sync" /></RecordingContext.Provider>);
    expect(screen.getByRole("button", { name: "Record: Weekly Sync" })).toBeInTheDocument();
  });

  it("appears when the meeting comes within ten minutes", () => {
    vi.setSystemTime(new Date("2026-11-27T14:45:00Z"));
    render(<RecordingContext.Provider value={recordingController()}><RecordMeetingButton event={event} /></RecordingContext.Provider>);
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    act(() => { vi.advanceTimersByTime(5 * 60_000 + 30_000); });
    expect(screen.getByRole("button", { name: "Record" })).toBeInTheDocument();
  });

  it("is absent for an ended meeting, while another recording runs, when recording is off, and where it is unsupported", () => {
    const ended = { ...event, start: "2026-11-27T13:00:00Z", end: "2026-11-27T14:00:00Z" };
    const view = (value: ReturnType<typeof recordingController>, target = event) => render(<RecordingContext.Provider value={value}><RecordMeetingButton event={target} /></RecordingContext.Provider>);
    view(recordingController(), ended);
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    cleanup();
    view(recordingController({ status: { ...recordingController().status, state: "recording", eventRef: null } }));
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    cleanup();
    view(recordingController({ enabled: false }));
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    cleanup();
    render(<RecordMeetingButton event={event} />);
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
});
