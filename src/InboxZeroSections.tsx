import { ContactRound, ListChecks } from "lucide-react";
import { useEffect, useState } from "react";
import { CalendarAgenda } from "./CalendarAgenda";
import { ContextRow, ContextSection } from "./ContextSections";
import { mailClient } from "./data/client";
import type { CalendarOption, ContactProfile, ScheduleEvent, ThreadTask } from "./domain";
import { ICON_SIZE } from "./iconSizes";
import { describeDue, isKeepInTouchDue } from "./keepInTouch";
import { compareTasksForDisplay, formatDue, isActiveTaskStatus, isOverdue } from "./taskViews";

/**
 * What's next once the inbox is empty: today's agenda, open tasks, and people
 * due to hear from you. Each section appears only when it has something to
 * show, and the agenda only when a calendar is connected, since calendars are
 * not tied to a mail account.
 */
export function InboxZeroSections({
  accountId, taskRefreshKey, calendarConnected, calendars,
  onOpenEvent, onEditTask, onOpenTasks, onOpenContact, onOpenKeepInTouch,
}: {
  /** The mail account in view, or null for all accounts; scopes tasks. */
  accountId: string | null;
  taskRefreshKey: number;
  calendarConnected: boolean;
  calendars: CalendarOption[];
  onOpenEvent(event: ScheduleEvent): void;
  onEditTask(task: ThreadTask): void;
  onOpenTasks(): void;
  onOpenContact(contactId: string): void;
  onOpenKeepInTouch(): void;
}) {
  const [tasks, setTasks] = useState<ThreadTask[]>([]);
  const [reconnect, setReconnect] = useState<ContactProfile[]>([]);

  useEffect(() => {
    let active = true;
    mailClient.listTasks(accountId ?? undefined).then((loaded) => {
      if (active) setTasks(loaded.filter((task) => isActiveTaskStatus(task.status)).sort(compareTasksForDisplay));
    }).catch(() => {
      // Supplemental to an empty inbox; a failed read leaves the section out.
      if (active) setTasks([]);
    });
    return () => { active = false; };
  }, [accountId, taskRefreshKey]);

  useEffect(() => {
    let active = true;
    mailClient.listKeepInTouch().then((profiles) => {
      if (!active) return;
      const due = profiles.filter((profile) => isKeepInTouchDue(profile));
      setReconnect(due.sort((left, right) => (left.keepInTouchDueAt ?? "").localeCompare(right.keepInTouchDueAt ?? "")));
    }).catch(() => {
      if (active) setReconnect([]);
    });
    return () => { active = false; };
  }, []);

  return <>
    {calendarConnected ? <CalendarAgenda today={new Date()} calendars={calendars} onOpenEvent={onOpenEvent} /> : null}
    {tasks.length > 0 ? <ContextSection
      id="inbox-zero-tasks"
      title="Open tasks"
      count={tasks.length}
      rows={tasks.map((task) => {
        const due = formatDue(task);
        return <ContextRow
          key={task.id}
          icon={<ListChecks size={ICON_SIZE.sm} />}
          title={task.title}
          wrapTitle
          date={due ? `Due ${due}` : undefined}
          dateClassName={due && isOverdue(task) ? "task-due-overdue" : undefined}
          detail={task.subjectSnapshot ?? undefined}
          onActivate={() => onEditTask(task)}
        />;
      })}
      footer={<button type="button" className="btn-link" onClick={onOpenTasks}>Open Tasks</button>}
    /> : null}
    {reconnect.length > 0 ? <ContextSection
      id="inbox-zero-keep-in-touch"
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
      footer={<button type="button" className="btn-link" onClick={onOpenKeepInTouch}>Open Keep in Touch</button>}
    /> : null}
  </>;
}
