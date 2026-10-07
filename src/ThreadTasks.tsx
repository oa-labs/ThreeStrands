import { Plus } from "lucide-react";
import { useEffect, useState } from "react";
import type { Thread, ThreadTask } from "./domain";
import { mailClient } from "./data/client";
import { HoverTooltip } from "./AppChrome";
import { errorMessage } from "./errors";
import { ContextRow, ContextSection } from "./ContextSections";
import { formatDue, isActiveTaskStatus, isCompletedToday, isDue, isOverdue } from "./taskViews";
import { ICON_SIZE } from "./iconSizes";

/**
 * Open tasks linked to the conversation, followed by open tasks from other
 * conversations with the selected person (`contactId`, when known). Tasks
 * marked done today stay listed, checked, so they can be unchecked until the
 * day ends. Renders nothing while there are none.
 */
export function ThreadTasks({ thread, contactId = null, refreshKey, onAddTask, onEditTask, onDraftFollowUp, onTasksChanged }: {
  /** The open conversation; null lists only the person's tasks, as while composing a new message. */
  thread: Thread | null;
  contactId?: string | null;
  refreshKey: number;
  /** Shows the Add control; tasks are added from a conversation. */
  onAddTask?(): void;
  onEditTask(task: ThreadTask): void;
  onDraftFollowUp(task: ThreadTask): void;
  onTasksChanged(): void;
}) {
  const [tasks, setTasks] = useState<ThreadTask[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [announcement, setAnnouncement] = useState("");

  const threadId = thread?.id ?? null;
  const accountId = thread?.accountId ?? null;
  useEffect(() => {
    let active = true;
    Promise.all([
      accountId ? mailClient.listTasks(accountId) : Promise.resolve([]),
      contactId ? mailClient.listContactTasks(contactId) : Promise.resolve([]),
    ])
      .then(([accountTasks, contactTasks]) => {
        if (!active) return;
        const now = new Date();
        const shown = (task: ThreadTask) => isActiveTaskStatus(task.status) || isCompletedToday(task, now);
        const here = accountTasks.filter((task) => task.threadId === threadId && shown(task));
        const elsewhere = contactTasks.filter((task) => task.threadId !== threadId && shown(task));
        setTasks([...here, ...elsewhere]);
        setError(null);
      })
      .catch((reason: unknown) => { if (active) setError(errorMessage(reason)); });
    return () => { active = false; };
  }, [accountId, contactId, refreshKey, threadId]);

  // Checking or unchecking updates the row in place so a done task stays where it was.
  const toggleDone = async (task: ThreadTask) => {
    const done = isActiveTaskStatus(task.status);
    try {
      const updated = await mailClient.setTaskStatus(task.id, done ? "completed" : "open");
      setTasks((current) => current.map((candidate) => candidate.id === task.id ? updated : candidate));
      setAnnouncement(`${done ? "Completed" : "Reopened"}: ${task.title}`);
      onTasksChanged();
    } catch (reason) {
      setError(errorMessage(reason));
    }
  };

  // An empty list is left out; adding a task (d) brings the section back. A
  // load failure keeps it so the error and the add control stay visible.
  return <>
    {tasks.length > 0 || error ? <ContextSection
      id="tasks"
      className="context-tasks"
      title="Tasks"
      label={thread ? "Conversation tasks" : "Tasks with this person"}
      count={tasks.filter((task) => isActiveTaskStatus(task.status)).length}
      actions={onAddTask ? <HoverTooltip title="Add task" shortcut="d" placement="bottom">
        <button type="button" className="btn-icon btn-icon-sm" aria-label="Add task" onClick={onAddTask}><Plus size={ICON_SIZE.sm} /></button>
      </HoverTooltip> : undefined}
      rows={tasks.map((task) => {
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
          // The date slot holds the due date; "Due" keeps it from reading as when the task was made.
          date={due ? `Due ${due}` : undefined}
          dateClassName={due && isOverdue(task) ? "task-due-overdue" : undefined}
          detail={task.threadId !== thread?.id && task.subjectSnapshot ? task.subjectSnapshot : undefined}
          onActivate={() => onEditTask(task)}
          trailing={task.kind === "follow_up" && isDue(task) ? <button type="button" className="btn btn-sm task-follow-up-button" onClick={() => onDraftFollowUp(task)}>Draft Follow-Up</button> : undefined}
        />;
      }).concat(error ? [<p className="form-error" role="alert" key="error">{error}</p>] : [])}
    /> : null}
    <p className="sr-only" aria-live="polite">{announcement}</p>
  </>;
}
