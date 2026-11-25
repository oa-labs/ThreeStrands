import { useState } from "react";
import { mailClient } from "./data/client";
import type { ThreadTask } from "./domain";
import { errorMessage } from "./errors";

/**
 * Says which email finished a completed task, and opens it when it is on this
 * device. The record comes from the send, so a message another device sent may
 * not have synced here yet; that is said rather than hidden. Shown only while
 * the task is completed, since another device can reopen it without clearing
 * the record.
 */
export function DoneByEmail({ task, onOpen }: {
  task: ThreadTask;
  onOpen(threadId: string, messageId: string): void;
}) {
  const [problem, setProblem] = useState<string | null>(null);
  const record = task.completedByEmail;
  if (task.status !== "completed" || !record) return null;
  const open = async () => {
    try {
      const threadId = record.messageId ? await mailClient.threadForMessage(record.accountId, record.messageId) : null;
      if (threadId && record.messageId) { setProblem(null); onOpen(threadId, record.messageId); }
      else setProblem("That message isn't on this device yet.");
    } catch (reason) {
      setProblem(errorMessage(reason));
    }
  };
  return (
    <span className="done-by-email">
      <small>Done by email</small>
      <button type="button" className="btn-link" onClick={() => void open()}>{record.subject.trim() || "(no subject)"}</button>
      {problem ? <small role="status">{problem}</small> : null}
    </span>
  );
}
