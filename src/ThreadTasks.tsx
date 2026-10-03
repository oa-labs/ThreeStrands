import { Check, Clock3, Plus } from "lucide-react";
import { useEffect, useState } from "react";
import type { Thread, ThreadTask } from "./domain";
import { mailClient } from "./data/client";
import { HoverTooltip } from "./AppChrome";
import { errorMessage } from "./errors";
import { ContextSection } from "./ContextSections";
import { formatDue, isActiveTaskStatus, isDue, isOverdue } from "./taskViews";

/**
 * Open tasks linked to the conversation, followed by open tasks from other
 * conversations with the selected person (`contactId`, when known). Renders
 * nothing while there are none.
 */
export function ThreadTasks({ thread, contactId = null, refreshKey, onAddTask, onEditTask, onDraftFollowUp, onTasksChanged }: {
  thread: Thread;
  contactId?: string | null;
  refreshKey: number;
  onAddTask(): void;
  onEditTask(task: ThreadTask): void;
  onDraftFollowUp(task: ThreadTask): void;
  onTasksChanged(): void;
}) {
  const [tasks, setTasks] = useState<ThreadTask[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [announcement, setAnnouncement] = useState("");

  useEffect(() => {
    let active = true;
    Promise.all([
      mailClient.listTasks(thread.accountId),
      contactId ? mailClient.listContactTasks(contactId) : Promise.resolve([]),
    ])
      .then(([accountTasks, contactTasks]) => {
        if (!active) return;
        const here = accountTasks.filter((task) => task.threadId === thread.id && isActiveTaskStatus(task.status));
        const elsewhere = contactTasks.filter((task) => task.threadId !== thread.id && isActiveTaskStatus(task.status));
        setTasks([...here, ...elsewhere]);
        setError(null);
      })
      .catch((reason: unknown) => { if (active) setError(errorMessage(reason)); });
    return () => { active = false; };
  }, [contactId, refreshKey, thread.accountId, thread.id]);

  const complete = async (task: ThreadTask) => {
    try {
      await mailClient.setTaskStatus(task.id, "completed");
      setTasks((current) => current.filter((candidate) => candidate.id !== task.id));
      setAnnouncement(`Completed: ${task.title}`);
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
      label="Conversation tasks"
      count={tasks.length}
      actions={<HoverTooltip title="Add task" shortcut="d" placement="bottom">
        <button type="button" className="context-icon-button" aria-label="Add task" onClick={onAddTask}><Plus size={15} /></button>
      </HoverTooltip>}
      rows={tasks.map((task) => {
        const due = formatDue(task);
        return <article className="context-task" key={task.id}>
          <button type="button" className="task-status-button" aria-label={`Complete ${task.title}`} onClick={() => void complete(task)}><Check size={15} /></button>
          <button type="button" className="context-task-main" onClick={() => onEditTask(task)}>
            <strong>{task.title}</strong>
            {task.threadId !== thread.id && task.subjectSnapshot ? <span className="context-task-source">{task.subjectSnapshot}</span> : null}
            {due ? <small className={isOverdue(task) ? "task-due-overdue" : undefined}><Clock3 size={12} /> {due}</small> : null}
          </button>
          {task.kind === "follow_up" && isDue(task) ? <button type="button" className="task-follow-up-button" onClick={() => onDraftFollowUp(task)}>Draft Follow-Up</button> : null}
        </article>;
      }).concat(error ? [<p className="form-error" role="alert" key="error">{error}</p>] : [])}
    /> : null}
    <p className="sr-only" aria-live="polite">{announcement}</p>
  </>;
}
