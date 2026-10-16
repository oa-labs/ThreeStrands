import { useCallback, useEffect, useMemo, useState, type RefObject } from "react";
import type { ScheduleEvent, ThreadDetail } from "./domain";
import type { ContactCardActions } from "./ContactCard";
import { readContactsView, writeContactsView, type ContactsView } from "./contactsView";
import { startOfLocalDay } from "./calendarTime";
import { useCalendarAccounts } from "./useCalendarAccounts";
import { mailClient } from "./data/client";
import { isKeepInTouchDue } from "./keepInTouch";
import { errorMessage, logBackgroundFailure } from "./errors";
import type { SettingsSection } from "./settingsPanelTypes";
import type { Notice } from "./useNotice";

export type RightWorkspace = "calendar" | "contacts" | "tasks" | "week" | "files" | null;
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
  const [rightWorkspace, setRightWorkspace] = useState<RightWorkspace>(null);
  const [calendarWeekAnchor, setCalendarWeekAnchor] = useState<Date | null>(null);
  const [calendarEventToOpen, setCalendarEventToOpen] = useState<ScheduleEvent | null>(null);
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
  const closeRightWorkspace = useCallback(() => setRightWorkspace(null), []);
  const calendar = useCalendarAccounts({ onLastAccountRemoved: closeRightWorkspace });
  const { refreshAccounts: refreshCalendarAccounts, refreshCalendars: refreshCalendarOptions } = calendar;
  useEffect(() => {
    if (settingsOpen && settingsSection === "calendarAccounts" && calendar.accounts.length > 0) {
      void refreshCalendarOptions().catch(logBackgroundFailure("Calendar listing"));
    }
  }, [calendar.accounts.length, refreshCalendarOptions, settingsOpen, settingsSection]);
  const openToday = useCallback(() => {
    if (rightWorkspace === "calendar") {
      setRightWorkspace(null);
      return;
    }
    setRightWorkspace(null);
    void refreshCalendarAccounts()
      .then((connected) => {
        if (!connected.some((account) => account.status === "connected")) {
          openSettingsAt("calendarAccounts");
          return;
        }
        setRightWorkspace("calendar");
      })
      .catch((reason: unknown) => {
        setNotice({ message: errorMessage(reason) });
      });
  }, [openSettingsAt, refreshCalendarAccounts, rightWorkspace, setNotice]);

  const openTasks = useCallback(() => {
    setRightWorkspace((current) => current === "tasks" ? null : "tasks");
  }, []);

  // Opening the Calendar sidebar on an event starts it on that day; the key
  // remounts the sidebar for each request.
  const calendarConnected = calendar.accounts.some((account) => account.status === "connected");
  const [calendarSidebarStart, setCalendarSidebarStart] = useState<{ key: number; date: Date } | null>(null);
  const openCalendarAt = useCallback((date: Date) => {
    setCalendarSidebarStart((current) => ({ key: (current?.key ?? 0) + 1, date }));
    setRightWorkspace("calendar");
  }, []);

  const openFilesView = useCallback(() => setRightWorkspace("files"), []);

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
      // Picking a person asks to see them, so bring the context panel back from another workspace.
      setRightWorkspace((current) => current === "tasks" || current === "week" || current === "contacts" || current === "files" ? null : current);
    },
    selectedEmail: contextPersonEmail,
  }), [openContactInAddressBook, visibleThreadId, contextPersonEmail, readingMessageIdRef]);
  const openCalendarView = useCallback((day?: Date, event?: ScheduleEvent) => {
    setCalendarWeekAnchor((current) => day ? startOfLocalDay(day) : current ?? startOfLocalDay(new Date()));
    setCalendarEventToOpen(event ?? null);
    setRightWorkspace("week");
    void refreshCalendarAccounts().catch(logBackgroundFailure("Calendar account listing"));
    void refreshCalendarOptions().catch(logBackgroundFailure("Calendar listing"));
  }, [refreshCalendarAccounts, refreshCalendarOptions]);

  return {
    rightWorkspace, setRightWorkspace, calendarWeekAnchor, setCalendarWeekAnchor,
    calendarEventToOpen, setCalendarEventToOpen, contactAddressBookTarget,
    setContactAddressBookTarget, contactsView, setContactsView, keepInTouchDueCount,
    refreshKeepInTouchCount, calendar, refreshCalendarOptions,
    calendarConnected, calendarSidebarStart, openCalendarAt, openToday, openTasks, openTasksView,
    openFilesView, openContactsView, openKeepInTouchView, openContactGroupsView, contactCardActions,
    contextPersonEmail, openCalendarView,
  };
}
