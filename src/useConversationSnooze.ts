import { useCallback } from "react";
import { mailClient } from "./data/client";
import type { CommandResult } from "./commands";
import type { Thread } from "./domain";
import { buildThreadMutation } from "./threadMutations";

/** The native batch is atomic; refresh via a ref supplied by the active view. */
/** `markRead` also marks snoozed conversations read; Undo restores the unread ones. */
export function useConversationSnooze(refresh: () => Promise<void>, markRead = false) {
  const snooze = useCallback(async (threads: Thread[], until: string): Promise<CommandResult> => {
    const before = await mailClient.listSnoozeRecords();
    const ids = threads.map((thread) => thread.id);
    const saved = await mailClient.snoozeThreads(ids, until);
    const restore = saved.map((record) => before.find((item) => item.id === record.id) ?? { ...record, state: "cancelled" as const });
    const archivedIds = threads.filter((thread) => thread.archived).map((thread) => thread.id);
    // Only after the snooze is saved, so a failed snooze changes nothing.
    const markedRead = markRead ? threads.filter((thread) => thread.unread).map((thread) => thread.id) : [];
    if (markedRead.length) {
      await mailClient.mutateThreads(markedRead.map((id) => buildThreadMutation(id, { kind: "read", value: true })));
    }
    await refresh();
    return {
      message: ids.length > 1 ? `Snoozed ${ids.length} conversations` : "Conversation snoozed",
      undoAction: async () => {
        await mailClient.restoreSnoozes(restore, archivedIds);
        if (markedRead.length) {
          await mailClient.mutateThreads(markedRead.map((id) => buildThreadMutation(id, { kind: "read", value: false })));
        }
        await refresh();
      },
    };
  }, [markRead, refresh]);
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
