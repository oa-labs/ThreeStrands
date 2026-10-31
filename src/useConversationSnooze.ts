import { useCallback } from "react";
import { mailClient } from "./data/client";
import type { CommandResult } from "./commands";
import type { Thread } from "./domain";

/** The native batch is atomic; refresh via a ref supplied by the active view. */
export function useConversationSnooze(refresh: () => Promise<void>) {
  const snooze = useCallback(async (threads: Thread[], until: string): Promise<CommandResult> => {
    const before = await mailClient.listSnoozeRecords();
    const ids = threads.map((thread) => thread.id);
    const saved = await mailClient.snoozeThreads(ids, until);
    const restore = saved.map((record) => before.find((item) => item.id === record.id) ?? { ...record, state: "cancelled" as const });
    const archivedIds = threads.filter((thread) => thread.archived).map((thread) => thread.id);
    await refresh();
    return {
      message: ids.length > 1 ? `Snoozed ${ids.length} conversations` : "Conversation snoozed",
      undoAction: async () => { await mailClient.restoreSnoozes(restore, archivedIds); await refresh(); },
    };
  }, [refresh]);
  const returnToInbox = useCallback(async (ids: string[]): Promise<CommandResult> => {
    const before = (await mailClient.listSnoozeRecords()).filter((record) => ids.includes(record.id));
    await mailClient.returnSnoozedThreads(ids);
    await refresh();
    return {
      message: ids.length > 1 ? `Returned ${ids.length} conversations to Inbox` : "Conversation returned to Inbox",
      undoAction: async () => { await mailClient.restoreSnoozes(before, []); await refresh(); },
    };
  }, [refresh]);
  return { snooze, returnToInbox };
}
