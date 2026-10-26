import { ContactRound, ListPlus, Mail, Send, Video, X } from "lucide-react";
import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { CalendarAgenda } from "./CalendarAgenda";
import { ContextRow, ContextSection } from "./ContextSections";
import { mailClient } from "./data/client";
import type { CalendarOption, ContactProfile, OpenPromise, ScheduleEvent, ThreadTask, TodayMail } from "./domain";
import { agendaEventsFor, formatClockTime, isDeclined } from "./agendaEvents";
import { addDays, eventDate, startOfLocalDay } from "./calendarTime";
import { parseAddress } from "./emailAddress";
import { errorMessage } from "./errors";
import { HoverTooltip } from "./AppChrome";
import { ICON_SIZE } from "./iconSizes";
import { describeDue, isKeepInTouchDue } from "./keepInTouch";
import { meetingPlace } from "./ThreadAssist";
import { dismissPromise, promiseRecipients, readDismissedPromises } from "./openPromises";
import { compareTasksForDisplay, dueView, formatDue, isActiveTaskStatus, isOverdue } from "./taskViews";
import { dayShape, meetingsWith, sharedMeetingsNote } from "./todayInsights";
import { groupTodayMail, type TodayMailGroup } from "./todayMail";
import { useCalendarSchedule } from "./useCalendarSchedule";

type Schedule = ReturnType<typeof useCalendarSchedule>;

export type TodayWorkspaceProps = {
  /** The mail account in view, or null for all accounts; scopes mail and tasks. */
  accountId: string | null;
  /** Changes when mail syncs, so the unread list is read again. */
  refreshKey: string;
  taskRefreshKey: number;
  calendarConnected: boolean;
  calendars: CalendarOption[];
  onOpenThread(threadId: string): void;
  onOpenEvent(event: ScheduleEvent): void;
  onEditTask(task: ThreadTask): void;
  onTasksChanged(): void;
  onOpenInbox(): void;
  onOpenCalendar(): void;
  onOpenTasks(): void;
  onOpenContact(contactId: string): void;
  onOpenKeepInTouch(): void;
};

const MAIL_GROUPS: { group: TodayMailGroup; title: string; limit: number }[] = [
  { group: "invitations", title: "Invitations", limit: 5 },
  { group: "reply", title: "Needs a reply", limit: 8 },
  { group: "people", title: "From people", limit: 8 },
  { group: "updates", title: "Updates", limit: 3 },
];

/**
 * The day at a glance: today's agenda, unread mail, and tasks due. The
 * agenda needs a connected calendar, since calendars are not tied to a mail
 * account; mail and tasks follow the account in view.
 */
export function TodayWorkspace(props: TodayWorkspaceProps) {
  return props.calendarConnected ? <ScheduledToday {...props} /> : <TodayLayout {...props} schedule={null} />;
}

function ScheduledToday(props: TodayWorkspaceProps) {
  const start = startOfLocalDay(new Date());
  const schedule = useCalendarSchedule({
    timeMin: start.toISOString(),
    // Through tomorrow, for the Tomorrow section under today's agenda.
    timeMax: addDays(start, 2).toISOString(),
    timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC",
  });
  return <TodayLayout {...props} schedule={schedule} />;
}

function TodayLayout({
  accountId, refreshKey, taskRefreshKey, calendars, schedule,
  onOpenThread, onOpenEvent, onEditTask, onTasksChanged, onOpenInbox, onOpenCalendar, onOpenTasks, onOpenContact, onOpenKeepInTouch,
}: TodayWorkspaceProps & { schedule: Schedule | null }) {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(new Date()), 60_000);
    return () => window.clearInterval(timer);
  }, []);

  const [mail, setMail] = useState<{ threads: TodayMail[]; total: number } | null>(null);
  const [mailError, setMailError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    mailClient.listTodayMail(accountId ?? undefined).then((page) => {
      if (!active) return;
      setMail(page);
      setMailError(null);
    }).catch((reason: unknown) => { if (active) setMailError(errorMessage(reason)); });
    return () => { active = false; };
  }, [accountId, refreshKey]);

  // Null until read, and after a failed read, so neither shows "No open tasks."
  const [loadedTasks, setTasks] = useState<ThreadTask[] | null>(null);
  const tasks = loadedTasks ?? [];
  const [announcement, setAnnouncement] = useState("");
  useEffect(() => {
    let active = true;
    mailClient.listTasks(accountId ?? undefined).then((loaded) => {
      if (active) setTasks(loaded.filter((task) => isActiveTaskStatus(task.status)).sort(compareTasksForDisplay));
    }).catch(() => {
      // Supplemental to the day's mail; a failed read leaves the column empty.
      if (active) setTasks(null);
    });
    return () => { active = false; };
  }, [accountId, taskRefreshKey]);

  const [reconnect, setReconnect] = useState<ContactProfile[]>([]);
  useEffect(() => {
    let active = true;
    mailClient.listKeepInTouch().then((profiles) => {
      if (!active) return;
      setReconnect(profiles.filter((profile) => isKeepInTouchDue(profile))
        .sort((left, right) => (left.keepInTouchDueAt ?? "").localeCompare(right.keepInTouchDueAt ?? "")));
    }).catch(() => { if (active) setReconnect([]); });
    return () => { active = false; };
  }, []);

  const [promises, setPromises] = useState<OpenPromise[]>([]);
  const [dismissed, setDismissed] = useState(readDismissedPromises);
  useEffect(() => {
    let active = true;
    mailClient.listOpenPromises(accountId ?? undefined).then((loaded) => {
      if (active) setPromises(loaded);
    }).catch(() => {
      // Supplemental, like tasks; a failed read leaves the section out.
      if (active) setPromises([]);
    });
    return () => { active = false; };
  }, [accountId, refreshKey, taskRefreshKey]);
  const openPromises = promises.filter((promise) => !dismissed.includes(promise.messageId));

  const trackPromise = async (promise: OpenPromise) => {
    try {
      const created = await mailClient.createTask({
        accountId: promise.thread.accountId,
        threadId: promise.thread.id,
        sourceMessageId: promise.messageId,
        subjectSnapshot: promise.thread.subject || "(no subject)",
        title: promise.excerpt,
        kind: "follow_up",
        evidenceText: promise.excerpt,
      });
      // The task now tracks it, so the native list leaves it out from here on.
      setPromises((current) => current.filter((candidate) => candidate.messageId !== promise.messageId));
      setAnnouncement(`Task added: ${created.title}`);
      onTasksChanged();
      onEditTask(created);
    } catch (reason) {
      setAnnouncement(errorMessage(reason));
    }
  };

  // Checking a task off keeps its row in place, struck through, until Today is read again.
  const toggleDone = async (task: ThreadTask) => {
    const done = isActiveTaskStatus(task.status);
    try {
      const updated = await mailClient.setTaskStatus(task.id, done ? "completed" : "open");
      setTasks((current) => current?.map((candidate) => candidate.id === task.id ? updated : candidate) ?? null);
      setAnnouncement(`${done ? "Completed" : "Reopened"}: ${task.title}`);
      onTasksChanged();
    } catch (reason) {
      setAnnouncement(errorMessage(reason));
    }
  };

  const todaysEvents = agendaEventsFor(schedule?.events ?? [], now, calendars);
  const next = nextMeeting(todaysEvents, now);
  const shape = schedule ? dayShape(todaysEvents, now) : "";
  const overdue = tasks.filter((task) => isActiveTaskStatus(task.status) && dueView(task, now) === "Overdue");
  const dueToday = tasks.filter((task) => dueView(task, now) === "Today");
  const open = tasks.filter((task) => !overdue.includes(task) && !dueToday.includes(task));
  const groups = groupTodayMail(mail?.threads ?? []);

  const summary = [
    schedule ? count(todaysEvents.filter((event) => !event.allDay && !isDeclined(event)).length, "meeting") : null,
    mail ? `${mail.total} unread` : null,
    count(overdue.length + dueToday.length, "task") + " due",
  ].filter(Boolean).join(" · ");

  const taskRow = (task: ThreadTask) => {
    const due = formatDue(task);
    const done = !isActiveTaskStatus(task.status);
    return <ContextRow
      as="article"
      key={task.id}
      className={done ? "context-task context-task-done" : "context-task"}
      control={<HoverTooltip label={done ? "Mark not done" : "Mark done"}>
        <input
          type="checkbox"
          className="context-task-checkbox"
          checked={done}
          aria-label={done ? `Mark ${task.title} not done` : `Mark ${task.title} done`}
          onChange={() => void toggleDone(task)}
        />
      </HoverTooltip>}
      title={task.title}
      wrapTitle
      date={due ? `Due ${due}` : undefined}
      dateClassName={due && isOverdue(task) ? "task-due-overdue" : undefined}
      detail={task.subjectSnapshot ?? undefined}
      onActivate={() => onEditTask(task)}
    />;
  };

  return <section className="today-workspace" aria-label="Today">
    <div className="today-page">
    <header className="today-header">
      <div>
        {/* Mail and tasks follow the account in view; the agenda shows every selected calendar, as Calendar does. */}
        <span className="eyebrow">Today · Mail and tasks: {accountId ?? "All accounts"}{schedule ? " · All calendars" : ""}</span>
        <h1>{now.toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric" })}</h1>
        <p className="today-summary">{summary}</p>
        {shape ? <p className="today-day-shape">{shape}</p> : null}
      </div>
      {next ? <NextMeeting event={next} now={now} onOpenEvent={onOpenEvent} /> : null}
    </header>
    <div className="today-columns">
      <div className="today-column" role="group" aria-label="Agenda">
        {schedule ? <>
          {/* The page heading already names today, so its agenda leaves the date out. */}
          <CalendarAgenda today={now} now={now} showDate={false} markConflicts calendars={calendars} schedule={schedule} onOpenEvent={onOpenEvent} />
          <CalendarAgenda today={addDays(startOfLocalDay(now), 1)} tomorrow calendars={calendars} schedule={schedule} onOpenEvent={onOpenEvent} />
        </>
          : <ContextSection id="calendar-agenda" title="Today's agenda" rows={[]}
            footer={<p className="context-status">Connect a calendar in Settings to see today's agenda here.</p>} />}
        <p className="today-column-link"><button type="button" className="btn-link" onClick={onOpenCalendar}>Open Calendar</button></p>
      </div>
      <div className="today-column" role="group" aria-label="Unread mail">
        {MAIL_GROUPS.map(({ group, title, limit }) => groups[group].length > 0 ? <ContextSection
          key={group}
          id={`today-mail-${group}`}
          title={title}
          count={groups[group].length}
          limit={limit}
          rows={groups[group].map((item) => {
            // Whoever wrote may also be in a meeting with the user later today.
            const shared = item.sender ? sharedMeetingsNote(meetingsWith(parseAddress(item.sender).email, todaysEvents, now)) : "";
            const subject = item.thread.subject || "(no subject)";
            return <ContextRow
              key={item.thread.id}
              icon={<Mail size={ICON_SIZE.sm} />}
              title={senderName(item)}
              date={formatArrival(item.thread.lastReceivedAt, now)}
              detail={shared ? <><span className="today-shared-meetings">{shared}</span> · {subject}</> : subject}
              onActivate={() => onOpenThread(item.thread.id)}
            />;
          })}
        /> : null)}
        {mailError || mail?.total === 0 ? <ContextSection id="today-mail-empty" title="Unread mail" rows={[]}
          footer={mailError ? <p className="form-error" role="alert">{mailError}</p> : <p className="context-status">Nothing unread.</p>} /> : null}
        {mail && mail.total > mail.threads.length ? <p className="context-status">Showing the newest {mail.threads.length} of {mail.total} unread.</p> : null}
        <p className="today-column-link"><button type="button" className="btn-link" onClick={onOpenInbox}>Open Inbox</button></p>
      </div>
      <div className="today-column" role="group" aria-label="Tasks">
        {openPromises.length > 0 ? <ContextSection
          id="today-promises"
          title="You said you would"
          count={openPromises.length}
          limit={5}
          rows={openPromises.map((promise) => <ContextRow
            as="article"
            key={promise.messageId}
            icon={<Send size={ICON_SIZE.sm} />}
            title={promise.excerpt}
            wrapTitle
            date={formatArrival(promise.sentAt, now)}
            detail={`To ${promiseRecipients(promise)} · Nothing sent to them since`}
            onActivate={() => onOpenThread(promise.thread.id)}
            actions={[
              <button key="task" type="button" className="btn-icon btn-icon-sm" aria-label={`Make a task: ${promise.excerpt}`} title="Make a task" onClick={() => void trackPromise(promise)}>
                <ListPlus size={ICON_SIZE.sm} />
              </button>,
              <button key="dismiss" type="button" className="btn-icon btn-icon-sm" aria-label={`Dismiss: ${promise.excerpt}`} title="Dismiss" onClick={() => setDismissed(dismissPromise(promise.messageId))}>
                <X size={ICON_SIZE.sm} />
              </button>,
            ]}
          />)}
        /> : null}
        {overdue.length > 0 ? <ContextSection id="today-tasks-overdue" title="Overdue" count={overdue.length} rows={overdue.map(taskRow)} /> : null}
        {dueToday.length > 0 ? <ContextSection id="today-tasks-due" title="Due today" count={dueToday.length} rows={dueToday.map(taskRow)} /> : null}
        {open.length > 0 ? <ContextSection id="today-tasks-open" title="Open tasks" count={open.length} limit={3} rows={open.map(taskRow)} /> : null}
        {loadedTasks?.length === 0 ? <ContextSection id="today-tasks-none" title="Open tasks" rows={[]} footer={<p className="context-status">No open tasks.</p>} /> : null}
        {reconnect.length > 0 ? <ContextSection
          id="today-keep-in-touch"
          title="Keep in touch"
          count={reconnect.length}
          rows={reconnect.map((profile) => <ContextRow
            key={profile.id}
            icon={<ContactRound size={ICON_SIZE.sm} />}
            title={profile.displayName || profile.addresses[0] || "Unnamed contact"}
            date={profile.keepInTouchDueAt ? describeDue(profile.keepInTouchDueAt) : undefined}
            detail={profile.displayName ? profile.addresses[0] : undefined}
            onActivate={() => onOpenContact(profile.id)}
          />)}
          closing={<button type="button" className="btn-link" onClick={onOpenKeepInTouch}>Open Keep in Touch</button>}
        /> : null}
        <p className="today-column-link"><button type="button" className="btn-link" onClick={onOpenTasks}>Open Tasks</button></p>
      </div>
    </div>
    </div>
    <p className="sr-only" aria-live="polite">{announcement}</p>
  </section>;
}

function NextMeeting({ event, now, onOpenEvent }: { event: ScheduleEvent; now: Date; onOpenEvent(event: ScheduleEvent): void }) {
  const { joinUrl } = meetingPlace(event.location ?? null, event.conferenceUrl);
  const minutes = Math.round((eventDate(event).getTime() - now.getTime()) / 60_000);
  const label = minutes <= 0
    ? `Now: ${event.title} · until ${formatClockTime(new Date(event.end))}`
    : `Next: ${event.title} ${minutes < 60 ? `in ${minutes} min` : `at ${formatClockTime(eventDate(event))}`}`;
  return <div className="today-next">
    <button type="button" className="btn-link" onClick={() => onOpenEvent(event)}>{label}</button>
    {joinUrl ? <button type="button" className="btn btn-sm" onClick={() => void openUrl(joinUrl)}><Video size={ICON_SIZE.sm} />Join</button> : null}
  </div>;
}

/** The timed meeting under way, or else the next one to start today; declined ones are skipped. */
export function nextMeeting(events: readonly ScheduleEvent[], now: Date): ScheduleEvent | null {
  return events
    .filter((event) => !event.allDay && !isDeclined(event) && new Date(event.end).getTime() > now.getTime())
    .sort((left, right) => eventDate(left).getTime() - eventDate(right).getTime())[0] ?? null;
}

function senderName(item: TodayMail): string {
  if (!item.sender) return item.thread.participants[0] ?? "Unknown sender";
  const parsed = parseAddress(item.sender);
  return parsed.name || parsed.email;
}

function formatArrival(iso: string, now: Date): string {
  const date = new Date(iso);
  return startOfLocalDay(date).getTime() === startOfLocalDay(now).getTime()
    ? formatClockTime(date)
    : date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

/** "No meetings", "1 meeting", "3 meetings". */
function count(total: number, noun: string): string {
  return total === 0 ? `No ${noun}s` : `${total} ${noun}${total === 1 ? "" : "s"}`;
}
