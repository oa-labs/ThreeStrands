import { ContactRound, Mail, Video } from "lucide-react";
import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { CalendarAgenda } from "./CalendarAgenda";
import { ContextRow, ContextSection } from "./ContextSections";
import { mailClient } from "./data/client";
import type { CalendarOption, ContactProfile, ScheduleEvent, ThreadTask, TodayMail } from "./domain";
import { addDays, eventDate, occursOnDay, startOfLocalDay } from "./calendarTime";
import { parseAddress } from "./emailAddress";
import { errorMessage } from "./errors";
import { HoverTooltip } from "./AppChrome";
import { ICON_SIZE } from "./iconSizes";
import { describeDue, isKeepInTouchDue } from "./keepInTouch";
import { meetingPlace } from "./ThreadAssist";
import { compareTasksForDisplay, dueView, formatDue, isActiveTaskStatus, isOverdue } from "./taskViews";
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
  onOpenTasks(): void;
  onOpenContact(contactId: string): void;
  onOpenKeepInTouch(): void;
};

const MAIL_GROUPS: { group: TodayMailGroup; title: string; limit: number }[] = [
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
  onOpenThread, onOpenEvent, onEditTask, onTasksChanged, onOpenInbox, onOpenTasks, onOpenContact, onOpenKeepInTouch,
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

  const [tasks, setTasks] = useState<ThreadTask[]>([]);
  const [announcement, setAnnouncement] = useState("");
  useEffect(() => {
    let active = true;
    mailClient.listTasks(accountId ?? undefined).then((loaded) => {
      if (active) setTasks(loaded.filter((task) => isActiveTaskStatus(task.status)).sort(compareTasksForDisplay));
    }).catch(() => {
      // Supplemental to the day's mail; a failed read leaves the column empty.
      if (active) setTasks([]);
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

  // Checking a task off keeps its row in place, struck through, until Today is read again.
  const toggleDone = async (task: ThreadTask) => {
    const done = isActiveTaskStatus(task.status);
    try {
      const updated = await mailClient.setTaskStatus(task.id, done ? "completed" : "open");
      setTasks((current) => current.map((candidate) => candidate.id === task.id ? updated : candidate));
      setAnnouncement(`${done ? "Completed" : "Reopened"}: ${task.title}`);
      onTasksChanged();
    } catch (reason) {
      setAnnouncement(errorMessage(reason));
    }
  };

  const selected = calendars.filter((calendar) => calendar.selected);
  const todaysEvents = (schedule?.events ?? []).filter((event) => occursOnDay(event, now) && selected.some((calendar) =>
    calendar.accountId === event.accountId && (!event.calendarId || calendar.id === event.calendarId)));
  const next = nextMeeting(todaysEvents, now);
  const overdue = tasks.filter((task) => isActiveTaskStatus(task.status) && dueView(task, now) === "Overdue");
  const dueToday = tasks.filter((task) => dueView(task, now) === "Today");
  const open = tasks.filter((task) => !overdue.includes(task) && !dueToday.includes(task));
  const groups = groupTodayMail(mail?.threads ?? []);

  const summary = [
    schedule ? plural(todaysEvents.filter((event) => !event.allDay).length, "meeting") : null,
    mail ? `${mail.total} unread` : null,
    plural(overdue.length + dueToday.length, "task") + " due",
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
    <header className="today-header">
      <div>
        <span className="eyebrow">Today · {accountId ?? "All accounts"}</span>
        <h1>{now.toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric" })}</h1>
        <p className="today-summary">{summary}</p>
      </div>
      {next ? <NextMeeting event={next} now={now} onOpenEvent={onOpenEvent} /> : null}
    </header>
    <div className="today-columns">
      <div className="today-column" role="group" aria-label="Agenda">
        {schedule ? <>
          <CalendarAgenda today={now} now={now} calendars={calendars} schedule={schedule} onOpenEvent={onOpenEvent} />
          <CalendarAgenda today={addDays(startOfLocalDay(now), 1)} tomorrow calendars={calendars} schedule={schedule} onOpenEvent={onOpenEvent} />
        </>
          : <p className="context-status">Connect a calendar in Settings to see today's agenda here.</p>}
      </div>
      <div className="today-column" role="group" aria-label="Unread mail">
        {MAIL_GROUPS.map(({ group, title, limit }) => groups[group].length > 0 ? <ContextSection
          key={group}
          id={`today-mail-${group}`}
          title={title}
          count={groups[group].length}
          limit={limit}
          rows={groups[group].map((item) => <ContextRow
            key={item.thread.id}
            icon={<Mail size={ICON_SIZE.sm} />}
            title={senderName(item)}
            date={formatArrival(item.thread.lastReceivedAt, now)}
            detail={item.thread.subject || "(no subject)"}
            onActivate={() => onOpenThread(item.thread.id)}
          />)}
        /> : null)}
        {mailError ? <p className="form-error" role="alert">{mailError}</p> : null}
        {mail && mail.total === 0 ? <p className="context-status">Nothing unread.</p> : null}
        {mail && mail.total > mail.threads.length ? <p className="context-status">Showing the newest {mail.threads.length} of {mail.total} unread.</p> : null}
        <p className="today-column-link"><button type="button" className="btn-link" onClick={onOpenInbox}>Open Inbox</button></p>
      </div>
      <div className="today-column" role="group" aria-label="Tasks">
        {overdue.length > 0 ? <ContextSection id="today-tasks-overdue" title="Overdue" count={overdue.length} rows={overdue.map(taskRow)} /> : null}
        {dueToday.length > 0 ? <ContextSection id="today-tasks-due" title="Due today" count={dueToday.length} rows={dueToday.map(taskRow)} /> : null}
        {open.length > 0 ? <ContextSection id="today-tasks-open" title="Open tasks" count={open.length} limit={3} rows={open.map(taskRow)} /> : null}
        {tasks.length === 0 ? <p className="context-status">No open tasks.</p> : null}
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
    <p className="sr-only" aria-live="polite">{announcement}</p>
  </section>;
}

function NextMeeting({ event, now, onOpenEvent }: { event: ScheduleEvent; now: Date; onOpenEvent(event: ScheduleEvent): void }) {
  const { joinUrl } = meetingPlace(event.location ?? null, event.conferenceUrl);
  const minutes = Math.round((eventDate(event).getTime() - now.getTime()) / 60_000);
  const when = minutes <= 0 ? "now" : minutes < 60 ? `in ${minutes} min` : `at ${eventDate(event).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}`;
  return <div className="today-next">
    <button type="button" className="btn-link" onClick={() => onOpenEvent(event)}>Next: {event.title} {when}</button>
    {joinUrl ? <button type="button" className="btn btn-sm" onClick={() => void openUrl(joinUrl)}><Video size={ICON_SIZE.sm} />Join</button> : null}
  </div>;
}

/** The timed meeting under way, or else the next one to start today. */
export function nextMeeting(events: readonly ScheduleEvent[], now: Date): ScheduleEvent | null {
  return events
    .filter((event) => !event.allDay && new Date(event.end).getTime() > now.getTime())
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
    ? date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })
    : date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function plural(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}
