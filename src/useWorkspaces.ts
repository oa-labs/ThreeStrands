import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from "react";
import type { ScheduleEvent, ThreadDetail } from "./domain";
import type { ContactCardActions } from "./ContactCard";
import { readContactsView, writeContactsView, type ContactsView } from "./contactsView";
import { readLaunchWorkspace, readSideColumn, writeLastWorkspace, writeSideColumn, type SideColumn } from "./lastWorkspace";
import { startOfLocalDay } from "./calendarTime";
import { useCalendarAccounts } from "./useCalendarAccounts";
import { mailClient } from "./data/client";
import { isKeepInTouchDue } from "./keepInTouch";
import { errorMessage, logBackgroundFailure } from "./errors";
import type { SettingsSection } from "./settingsPanelTypes";
import type { Notice } from "./useNotice";
import { NARROW_LAYOUT_QUERY, useMediaQuery } from "./useMediaQuery";

export type RightWorkspace = "contacts" | "tasks" | "notes" | "week" | "files" | "today" | "agents" | null;
/** A request for the Notes workspace to carry out once it is open; `id` tells repeats apart. */
export type NotesRequest = { kind: "today"; id: number } | { kind: "note"; noteId: string; markerId?: string; referenceHref?: string; id: number };
// Keep-in-touch reminders fall due as time passes and as mail arrives,
// without any local edit, so the Contacts badge is re-read on this cadence.
const KEEP_IN_TOUCH_REFRESH_MS = 5 * 60_000;

type Options = {
  settingsOpen: boolean;
  settingsSection: SettingsSection;
  visibleDetail: ThreadDetail | null;
  /** The message the reader is reading, which a person picked from it belongs to. */
  readingMessageId: string | null;
  readingMessageIdRef: RefObject<string | null>;
  openSettingsAt: (section: SettingsSection) => void;
  setNotice: (notice: Notice | null) => void;
};

/** Owns the selected workspace and calendar/contact entry points. */
export function useWorkspaces({ settingsOpen, settingsSection, visibleDetail, readingMessageId, readingMessageIdRef, openSettingsAt, setNotice }: Options) {
  const [rightWorkspace, setRightWorkspace] = useState<RightWorkspace>(readLaunchWorkspace);
  useEffect(() => { writeLastWorkspace(rightWorkspace); }, [rightWorkspace]);
  const [sideColumn, setSideColumn] = useState<SideColumn>(readSideColumn);
  useEffect(() => { writeSideColumn(sideColumn); }, [sideColumn]);
  // In a narrow window the right column is a drawer over the reader. It starts
  // closed each time the window becomes narrow.
  const narrowLayout = useMediaQuery(NARROW_LAYOUT_QUERY);
  const [drawerRequested, setDrawerRequested] = useState(false);
  useEffect(() => { setDrawerRequested(false); }, [narrowLayout]);
  const sideDrawerOpen = narrowLayout && drawerRequested;
  const [calendarWeekAnchor, setCalendarWeekAnchor] = useState<Date | null>(null);
  const [calendarEventToOpen, setCalendarEventToOpen] = useState<ScheduleEvent | null>(null);
  // Event jumps preserve the underlying reader; Escape only needs to restore its workspace.
  const [calendarReturnWorkspace, setCalendarReturnWorkspace] = useState<{ workspace: RightWorkspace } | null>(null);
  useEffect(() => {
    if (rightWorkspace !== "week") setCalendarReturnWorkspace(null);
  }, [rightWorkspace]);
  const canReturnFromCalendar = rightWorkspace === "week" && calendarReturnWorkspace !== null;
  const returnFromCalendar = useCallback(() => {
    if (!canReturnFromCalendar || !calendarReturnWorkspace) return;
    setRightWorkspace(calendarReturnWorkspace.workspace);
    setCalendarReturnWorkspace(null);
  }, [canReturnFromCalendar, calendarReturnWorkspace]);
  const [contactAddressBookTarget, setContactAddressBookTarget] = useState<string | null>(null);
  const [contactsView, setContactsView] = useState<ContactsView>(readContactsView);
  useEffect(() => { writeContactsView(contactsView); }, [contactsView]);
  const [keepInTouchDueCount, setKeepInTouchDueCount] = useState(0);
  const refreshKeepInTouchCount = useCallback(async () => {
    try {
      setKeepInTouchDueCount((await mailClient.listKeepInTouch()).filter((profile) => isKeepInTouchDue(profile)).length);
    } catch {
      // The Contacts badge is supplemental; mail remains usable if unavailable.
    }
  }, []);
  useEffect(() => {
    void refreshKeepInTouchCount();
    const timer = window.setInterval(() => void refreshKeepInTouchCount(), KEEP_IN_TOUCH_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [refreshKeepInTouchCount]);
  const showContextColumn = useCallback(() => setSideColumn("context"), []);
  const calendar = useCalendarAccounts({ onLastAccountRemoved: showContextColumn });
  const { refreshAccounts: refreshCalendarAccounts, refreshCalendars: refreshCalendarOptions } = calendar;
  // Replicated sync can add or remove calendar accounts while the app runs,
  // so re-read them each time Settings opens, as mail accounts are.
  useEffect(() => {
    if (settingsOpen) void refreshCalendarAccounts().catch(logBackgroundFailure("Calendar account listing"));
  }, [refreshCalendarAccounts, settingsOpen]);
  useEffect(() => {
    if (settingsOpen && settingsSection === "calendarAccounts" && calendar.accounts.length > 0) {
      void refreshCalendarOptions().catch(logBackgroundFailure("Calendar listing"));
    }
  }, [calendar.accounts.length, refreshCalendarOptions, settingsOpen, settingsSection]);

  const openTasks = useCallback(() => {
    setRightWorkspace((current) => current === "tasks" ? null : "tasks");
  }, []);

  // Opening the Calendar sidebar on an event starts it on that day; the key
  // remounts the sidebar for each request.
  const calendarConnected = calendar.accounts.some((account) => account.status === "connected");
  // A remembered schedule needs a connected calendar; without one the column shows the context.
  const scheduleShown = sideColumn === "schedule" && calendarConnected;
  /**
   * Swaps Mail's right column between the context and the schedule. From
   * another view it returns to Mail with the schedule showing, and with no
   * calendar connected it opens Calendar Accounts instead.
   */
  const toggleSchedule = useCallback(() => {
    const inMail = rightWorkspace === null;
    setRightWorkspace(null);
    // A closed drawer opens on the schedule rather than swapping what it hides.
    if (inMail && scheduleShown && (!narrowLayout || drawerRequested)) {
      setSideColumn("context");
      return;
    }
    void refreshCalendarAccounts()
      .then((connected) => {
        if (!connected.some((account) => account.status === "connected")) {
          openSettingsAt("calendarAccounts");
          return;
        }
        setSideColumn("schedule");
        setDrawerRequested(true);
      })
      .catch((reason: unknown) => {
        setNotice({ message: errorMessage(reason) });
      });
  }, [drawerRequested, narrowLayout, openSettingsAt, refreshCalendarAccounts, rightWorkspace, scheduleShown, setNotice]);
  /** Brings Mail back with the context column, for actions that show something in it. */
  const showMailContext = useCallback(() => {
    setRightWorkspace(null);
    setSideColumn("context");
    setDrawerRequested(true);
  }, []);
  const [calendarSidebarStart, setCalendarSidebarStart] = useState<{ key: number; date: Date } | null>(null);
  const openCalendarAt = useCallback((date: Date) => {
    setCalendarSidebarStart((current) => ({ key: (current?.key ?? 0) + 1, date }));
    setRightWorkspace(null);
    setSideColumn("schedule");
    setDrawerRequested(true);
  }, []);

  const openAgentsView = useCallback(() => setRightWorkspace("agents"), []);
  const openFilesView = useCallback(() => setRightWorkspace("files"), []);
  const openNotesView = useCallback(() => setRightWorkspace("notes"), []);
  const [notesRequest, setNotesRequest] = useState<NotesRequest | null>(null);
  const notesRequestId = useRef(0);
  const consumeNotesRequest = useCallback((id: number) => {
    setNotesRequest((current) => current?.id === id ? null : current);
  }, []);
  const openTodaysNote = useCallback(() => {
    setNotesRequest({ kind: "today", id: ++notesRequestId.current });
    setRightWorkspace("notes");
  }, []);
  const openNote = useCallback((noteId: string, markerId?: string, referenceHref?: string) => {
    setNotesRequest({ kind: "note", noteId, markerId, referenceHref, id: ++notesRequestId.current });
    setRightWorkspace("notes");
  }, []);
  const openTodayView = useCallback(() => setRightWorkspace("today"), []);

  const openTasksView = useCallback(() => {
    setRightWorkspace("tasks");
  }, []);

  const openContactsView = useCallback(() => { setContactAddressBookTarget(null); setRightWorkspace(current => current === "contacts" ? null : "contacts"); }, []);
  const openKeepInTouchView = useCallback(() => { setContactAddressBookTarget(null); setContactsView("keepInTouch"); setRightWorkspace("contacts"); }, []);
  const openContactGroupsView = useCallback(() => { setContactAddressBookTarget(null); setContactsView("groups"); setRightWorkspace("contacts"); }, []);
  const openContactInAddressBook = useCallback((id: string) => { setContactAddressBookTarget(id); setContactsView("all"); setRightWorkspace("contacts"); }, []);
  // The participant picked from a message header or the panel's person
  // switcher. It holds while the reader stays on the message it was picked
  // from; moving to another message, or another conversation, returns the
  // panel to that message's person.
  const [contextPersonPick, setContextPersonPick] = useState<{ threadId: string; messageId: string | null; email: string } | null>(null);
  const contextPersonEmail = visibleDetail && contextPersonPick?.threadId === visibleDetail.thread.id
    && contextPersonPick.messageId === readingMessageId ? contextPersonPick.email : null;
  const visibleThreadId = visibleDetail?.thread.id ?? null;
  const contactCardActions = useMemo<ContactCardActions>(() => ({
    onOpenContact: openContactInAddressBook,
    onSelectPerson: (email) => {
      if (!visibleThreadId) return;
      setContextPersonPick({ threadId: visibleThreadId, messageId: readingMessageIdRef.current, email });
      // Picking a person asks to see them, so bring the context column back.
      showMailContext();
    },
    selectedEmail: contextPersonEmail,
  }), [openContactInAddressBook, visibleThreadId, contextPersonEmail, readingMessageIdRef, showMailContext]);
  const openCalendarView = useCallback((day?: Date, event?: ScheduleEvent) => {
    // Opening an event is a reversible jump; choosing Calendar itself is navigation.
    setCalendarReturnWorkspace(event && rightWorkspace !== "week" ? { workspace: rightWorkspace } : null);
    setCalendarWeekAnchor((current) => day ? startOfLocalDay(day) : current ?? startOfLocalDay(new Date()));
    setCalendarEventToOpen(event ?? null);
    setRightWorkspace("week");
    void refreshCalendarAccounts().catch(logBackgroundFailure("Calendar account listing"));
    void refreshCalendarOptions().catch(logBackgroundFailure("Calendar listing"));
  }, [refreshCalendarAccounts, refreshCalendarOptions, rightWorkspace]);

  return {
    rightWorkspace, setRightWorkspace, calendarWeekAnchor, setCalendarWeekAnchor,
    calendarEventToOpen, setCalendarEventToOpen, contactAddressBookTarget,
    setContactAddressBookTarget, contactsView, setContactsView, keepInTouchDueCount,
    refreshKeepInTouchCount, calendar, refreshCalendarOptions,
    calendarConnected, calendarSidebarStart, openCalendarAt, toggleSchedule, openTasks,
    sideColumn, setSideColumn, scheduleShown, showMailContext, openTasksView,
    narrowLayout, sideDrawerOpen, setSideDrawerOpen: setDrawerRequested,
    openAgentsView, openFilesView, openNotesView, openTodaysNote, openNote, notesRequest, consumeNotesRequest, openTodayView, openContactsView, openKeepInTouchView, openContactGroupsView, contactCardActions,
    contextPersonEmail, openCalendarView, canReturnFromCalendar, returnFromCalendar,
  };
}
