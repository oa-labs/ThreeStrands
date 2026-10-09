import { useCallback, useEffect, useMemo, useState } from "react";
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

export type RightWorkspace = "calendar" | "contacts" | "tasks" | "week" | null;
// Keep-in-touch reminders fall due as time passes and as mail arrives,
// without any local edit, so the Contacts badge is re-read on this cadence.
const KEEP_IN_TOUCH_REFRESH_MS = 5 * 60_000;

type Options = {
  settingsOpen: boolean;
  settingsSection: SettingsSection;
  visibleDetail: ThreadDetail | null;
  openSettingsAt: (section: SettingsSection) => void;
  setNotice: (notice: Notice | null) => void;
};

/** Owns the selected workspace and calendar/contact entry points. */
export function useWorkspaces({ settingsOpen, settingsSection, visibleDetail, openSettingsAt, setNotice }: Options) {
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

  // Opening the Calendar sidebar from a meeting starts it on that day at the
  // meeting's duration; the key remounts the sidebar for each request.
  const calendarConnected = calendar.accounts.some((account) => account.status === "connected");
  const [calendarSidebarStart, setCalendarSidebarStart] = useState<{ key: number; date: Date; durationMinutes: number } | null>(null);
  const openCalendarAt = useCallback((date: Date, durationMinutes: number) => {
    setCalendarSidebarStart((current) => ({ key: (current?.key ?? 0) + 1, date, durationMinutes }));
    setRightWorkspace("calendar");
  }, []);

  const openTasksView = useCallback(() => {
    setRightWorkspace("tasks");
  }, []);

  const openContactsView = useCallback(() => { setContactAddressBookTarget(null); setRightWorkspace(current => current === "contacts" ? null : "contacts"); }, []);
  const openKeepInTouchView = useCallback(() => { setContactAddressBookTarget(null); setContactsView("keepInTouch"); setRightWorkspace("contacts"); }, []);
  const openContactGroupsView = useCallback(() => { setContactAddressBookTarget(null); setContactsView("groups"); setRightWorkspace("contacts"); }, []);
  const openContactInAddressBook = useCallback((id: string) => { setContactAddressBookTarget(id); setContactsView("all"); setRightWorkspace("contacts"); }, []);
  // The participant picked from a message header, kept per conversation so
  // opening another conversation returns the panel to its latest sender.
  const [contextPersonPick, setContextPersonPick] = useState<{ threadId: string; email: string } | null>(null);
  const contextPersonEmail = visibleDetail && contextPersonPick?.threadId === visibleDetail.thread.id ? contextPersonPick.email : null;
  const visibleThreadId = visibleDetail?.thread.id ?? null;
  const contactCardActions = useMemo<ContactCardActions>(() => ({
    onOpenContact: openContactInAddressBook,
    onSelectPerson: (email) => {
      if (!visibleThreadId) return;
      setContextPersonPick({ threadId: visibleThreadId, email });
      // Picking a person asks to see them, so bring the context panel back from another workspace.
      setRightWorkspace((current) => current === "tasks" || current === "week" || current === "contacts" ? null : current);
    },
    selectedEmail: contextPersonEmail,
  }), [openContactInAddressBook, visibleThreadId, contextPersonEmail]);
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
    openContactsView, openKeepInTouchView, openContactGroupsView, contactCardActions,
    contextPersonEmail, openCalendarView,
  };
}
