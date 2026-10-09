import { useCallback, useEffect, useState } from "react";
import { mailClient } from "./data/client";
import { isActiveTaskStatus } from "./taskViews";
import { logBackgroundFailure } from "./errors";

const TASK_RECONCILE_INTERVAL_MS = 60_000;

export function useTaskIndicators(activeAccountId: string | null) {
  const [openTaskThreadIds, setOpenTaskThreadIds] = useState<Set<string>>(new Set());
  useEffect(() => {
    void mailClient.reconcileTasks().catch(logBackgroundFailure("Task reconciliation"));
  }, []);
  const [taskRevision, setTaskRevision] = useState(0);
  const refreshTaskIndicators = useCallback(async () => {
    try {
      const tasks = await mailClient.listTasks(activeAccountId ?? undefined);
      setOpenTaskThreadIds(new Set(tasks.flatMap((task) => task.threadId && isActiveTaskStatus(task.status) ? [task.threadId] : [])));
    } catch {
      // Task indicators are supplemental; mail remains usable if unavailable.
    }
  }, [activeAccountId]);
  useEffect(() => { void refreshTaskIndicators(); }, [refreshTaskIndicators]);
  useEffect(() => {
    const timer = window.setInterval(() => {
      void mailClient.reconcileTasks().then(() => refreshTaskIndicators()).catch(logBackgroundFailure("Task reconciliation"));
    }, TASK_RECONCILE_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [refreshTaskIndicators]);
  return { openTaskThreadIds, taskRevision, setTaskRevision, refreshTaskIndicators };
}
