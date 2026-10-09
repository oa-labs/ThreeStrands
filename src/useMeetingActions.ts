import { useCallback, useEffect, useRef, useState } from "react";
import type {
  ActionProposal,
  AvailabilityCandidate,
  CalendarEventPreview,
  MeetingProposal,
  OpenedCalendarFile,
  ScheduleEvent,
  ThreadDetail,
} from "./domain";
import { formatAvailabilityText, formatConfirmationText } from "./actionDrafting";
import type { ScheduleSlot } from "./MeetingScheduler";
import { proactiveBriefSender } from "./proactiveBrief";
import { listenForOpenedCalendarFiles } from "./calendarFiles";
import { calendarEventRange } from "./CalendarAttachment";
import { calendarDescriptionText } from "./calendarDescription";
import { revalidateScheduleCache } from "./calendarScheduleCache";
import { eventDate, startOfLocalDay } from "./calendarTime";
import { logBackgroundFailure } from "./errors";
import type { useWorkspaces } from "./useWorkspaces";
import type { useCorrespondence } from "./useCorrespondence";
import type { useAppPreferences } from "./useAppPreferences";
import type { ProposalSource, useThreadIntelligence } from "./useThreadIntelligence";
import type { SettingsSection } from "./settingsPanelTypes";
import type { Notice } from "./useNotice";

type MeetingEditorState = { index: number; proposal: MeetingProposal };

type Options = Pick<ReturnType<typeof useThreadIntelligence>, "ownAddresses" | "removeActionProposal"> & {
  workspaces: Pick<ReturnType<typeof useWorkspaces>, "setRightWorkspace" | "refreshCalendarOptions" | "calendarConnected" | "setCalendarWeekAnchor" | "setCalendarEventToOpen" | "openCalendarAt">;
  correspondence: Pick<ReturnType<typeof useCorrespondence>, "activeDraft" | "liveDraft" | "insertIntoDraft" | "replyWithAvailability" | "replyWithText">;
  visibleDetail: ThreadDetail | null;
  availabilityPreferences: ReturnType<typeof useAppPreferences>["availabilityPreferences"];
  openSettingsAt: (section: SettingsSection) => void;
  setNotice: (notice: Notice | null) => void;
};

export function useMeetingActions({
  workspaces, correspondence, visibleDetail, availabilityPreferences, openSettingsAt, setNotice,
  ownAddresses, removeActionProposal,
}: Options) {
  const {
    setRightWorkspace, refreshCalendarOptions, calendarConnected, setCalendarWeekAnchor,
    setCalendarEventToOpen, openCalendarAt,
  } = workspaces;
  const [meetingEditor, setMeetingEditor] = useState<MeetingEditorState | null>(null);
  const draftAvailabilityReply = useCallback((candidates: AvailabilityCandidate[]) => {
    if (candidates.length === 0) return;
    const text = formatAvailabilityText(candidates, availabilityPreferences.timeZone);
    // An open draft, new or a reply, takes the times at the caret.
    if (correspondence.activeDraft) correspondence.insertIntoDraft(text);
    else correspondence.replyWithAvailability(text, visibleDetail?.messages.at(-1)?.id);
    setRightWorkspace(null);
  }, [availabilityPreferences.timeZone, correspondence, setRightWorkspace, visibleDetail?.messages]);

  // Add to Calendar from a meeting suggestion or a chat answer: the event
  // dialog opens prefilled and nothing is created until the user submits.
  const [meetingEventDraft, setMeetingEventDraft] = useState<{
    start: Date;
    end: Date;
    title: string;
    invitees: string[];
    description: string;
    /** The suggestion this event comes from; it is removed once the event exists. */
    source: { from: ProposalSource; proposal: ActionProposal } | null;
    /** Added from an opened calendar file, which is dismissed once the event exists. */
    fromOpenedFile?: boolean;
  } | null>(null);
  // .ics files macOS opened with ThreeStrands, shown one at a time.
  const [openedCalendarFiles, setOpenedCalendarFiles] = useState<{ id: number; file: OpenedCalendarFile }[]>([]);
  const nextOpenedCalendarFileId = useRef(0);
  useEffect(() => listenForOpenedCalendarFiles((files) => {
    const arrived = files.map((file) => ({ id: nextOpenedCalendarFileId.current++, file }));
    setOpenedCalendarFiles((current) => [...current, ...arrived]);
  }), []);
  const dismissOpenedCalendarFile = useCallback(() => setOpenedCalendarFiles((current) => current.slice(1)), []);
  // The invitation is added as the user's own event, without its guests, so
  // nobody on the original invitation is emailed again.
  const addOpenedInvitationToCalendar = useCallback((event: CalendarEventPreview) => {
    const range = calendarEventRange(event);
    if (!range) return;
    void refreshCalendarOptions().catch(logBackgroundFailure("Calendar listing"));
    setMeetingEventDraft({
      ...range,
      title: event.title,
      invitees: [],
      description: [
        event.location ? `Location: ${event.location}` : null,
        event.organizer ? `Organized by ${event.organizer}` : null,
        event.description ? calendarDescriptionText(event.description) : null,
      ].filter(Boolean).join("\n\n"),
      source: null,
      fromOpenedFile: true,
    });
  }, [refreshCalendarOptions]);
  const addMeetingToCalendar = useCallback((slot: ScheduleSlot, meeting: { title: string; participants: string[]; excerpt: string | null }, source: { from: ProposalSource; proposal: ActionProposal } | null) => {
    if (!visibleDetail) return;
    const invitees = meeting.participants.filter((participant) => /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(participant.trim())).map((participant) => participant.trim());
    const sender = proactiveBriefSender(visibleDetail, ownAddresses);
    // The dialog lists writable calendars, which load lazily elsewhere.
    void refreshCalendarOptions().catch(logBackgroundFailure("Calendar listing"));
    setMeetingEventDraft({
      start: new Date(slot.start),
      end: new Date(slot.end),
      title: meeting.title,
      invitees: invitees.length > 0 ? invitees : sender ? [sender] : [],
      description: [`Scheduled from “${visibleDetail.thread.subject}”.`, meeting.excerpt ? `“${meeting.excerpt}”` : null].filter(Boolean).join("\n\n"),
      source,
    });
  }, [ownAddresses, refreshCalendarOptions, visibleDetail]);
  // Add to Calendar from the compose panel's open times: the draft's
  // recipients are invited and its subject names the meeting.
  const addComposeMeeting = useCallback((slot: ScheduleSlot, invitees: string[]) => {
    void refreshCalendarOptions().catch(logBackgroundFailure("Calendar listing"));
    setMeetingEventDraft({
      start: new Date(slot.start),
      end: new Date(slot.end),
      title: correspondence.liveDraft?.subject.trim() ?? "",
      invitees,
      description: "",
      source: null,
    });
  }, [correspondence.liveDraft?.subject, refreshCalendarOptions]);
  const meetingCreated = useCallback((event: ScheduleEvent) => {
    const source = meetingEventDraft?.source;
    if (source) removeActionProposal(source.from, source.proposal, event);
    setMeetingEventDraft(null);
    revalidateScheduleCache();
    setCalendarWeekAnchor(startOfLocalDay(eventDate(event)));
    setCalendarEventToOpen(event);
    setRightWorkspace("week");
    void refreshCalendarOptions().catch(logBackgroundFailure("Calendar listing"));
    setNotice({ message: "Added to calendar" });
  }, [
    meetingEventDraft?.source, refreshCalendarOptions, removeActionProposal,
    setCalendarEventToOpen, setCalendarWeekAnchor, setNotice, setRightWorkspace,
  ]);
  const confirmMeetingTime = useCallback((slot: ScheduleSlot) => {
    correspondence.replyWithText(formatConfirmationText(slot, availabilityPreferences.timeZone), visibleDetail?.messages.at(-1)?.id);
  }, [availabilityPreferences.timeZone, correspondence, visibleDetail]);
  const meetingScheduling = {
    calendarConnected,
    preferences: availabilityPreferences,
    onReplyWithTimes: draftAvailabilityReply,
    onConfirmTime: confirmMeetingTime,
    onMoreTimes: openCalendarAt,
    onOpenCalendarSettings: () => openSettingsAt("calendarAccounts"),
  };

  return {
    meetingEditor, setMeetingEditor, meetingEventDraft, setMeetingEventDraft, openedCalendarFiles,
    dismissOpenedCalendarFile, addOpenedInvitationToCalendar, addMeetingToCalendar,
    addComposeMeeting, meetingCreated, meetingScheduling, draftAvailabilityReply,
  };
}
