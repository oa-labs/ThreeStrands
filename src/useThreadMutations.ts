import { useCallback, useEffect, useRef, type Dispatch, type SetStateAction } from "react";
import type { CommandResult, MailboxKind } from "./commands";
import { mailClient } from "./data/client";
import type { ConversationSnooze, SyncStatus, Thread, ThreadDetail, TriageEvent } from "./domain";
import { logBackgroundFailure } from "./errors";
import {
  applyMutationTemplate,
  buildThreadMutation,
  describeMutation,
  invertMutationTemplate,
  type MutationTemplate,
} from "./threadMutations";
import type { AfterTriage } from "./settings";
import { STARRED_INBOX_ID } from "./starredInbox";
import { sortByRecency, triageNow } from "./threadPresentation";
import { buildTriageDispositionEvent } from "./triage";
import type { useMailboxThreads } from "./useMailboxThreads";
import type { Notice } from "./useNotice";
import type { useTriageSession } from "./useTriageSession";

type Options = ReturnType<typeof useTriageSession> & Pick<ReturnType<typeof useMailboxThreads>, "loadThreadsRef" | "queryRef" | "setThreads"> & {
  threads: Thread[];
  detail: ThreadDetail | null;
  visibleDetail: ThreadDetail | null;
  selectedId: string | null;
  setSelectedId: Dispatch<SetStateAction<string | null>>;
  setDetail: Dispatch<SetStateAction<ThreadDetail | null>>;
  setCheckedIds: Dispatch<SetStateAction<Set<string>>>;
  setSyncStatus: Dispatch<SetStateAction<SyncStatus | null>>;
  setNotice: (notice: Notice | null) => void;
  refreshMailboxUnreadCounts(): void;
  mailbox: MailboxKind;
  activeAccountId: string | null;
  activeSplitInboxId: string | null;
  includeArchived: boolean;
  isThreadMailbox: boolean;
  autoReadDelaySeconds: number;
  /** False leaves opened conversations unread until the user marks them. */
  markReadOnOpen?: boolean;
  markReadOnArchive: boolean;
  /** Moving to Trash also marks the conversations read. */
  markReadOnTrash?: boolean;
  /** Which conversation opens after the open one leaves the list. */
  afterTriage?: AfterTriage;
};

/** Applies optimistic mail changes, rollback and undo within the originating view. */
export function useThreadMutations({
  threads, detail, visibleDetail, selectedId, setSelectedId, setDetail, setThreads, setCheckedIds,
  setSyncStatus, setNotice, mailbox, activeAccountId, activeSplitInboxId, includeArchived,
  isThreadMailbox, autoReadDelaySeconds, markReadOnOpen = true, markReadOnArchive, markReadOnTrash = false, afterTriage = "older", triageSessionRef, recordTriageEvent, loadThreadsRef,
  queryRef, refreshMailboxUnreadCounts,
}: Options) {
  const autoReadSuppressedForId = useRef<string | null>(null);
  // Identifies the thread list on screen, so work that outlives a view switch
  // (a mutation's round trip, a later Undo) can tell whether to touch it.
  const viewKey = [mailbox, activeAccountId ?? "", activeSplitInboxId ?? "", includeArchived].join("\u0000");
  const viewKeyRef = useRef(viewKey);
  viewKeyRef.current = viewKey;

  /**
   * Applies `template` to the conversations. By default only conversations in
   * the loaded list count; `includeUnloaded` also reaches ones the list has
   * not paged in, such as every Inbox conversation from one sender.
   */
  const mutateIds = useCallback(async (ids: string[], template: MutationTemplate, options?: { includeUnloaded?: boolean }): Promise<CommandResult> => {
    const targetIds = options?.includeUnloaded
      ? [...new Set(ids)]
      : ids.filter((id) => threads.some((thread) => thread.id === id));
    if (targetIds.length === 0) return {};
    if (template.kind === "read" && !template.value && selectedId && targetIds.includes(selectedId)) {
      autoReadSuppressedForId.current = selectedId;
    }
    const triageContext: TriageEvent["context"] = mailbox === "inbox" && !includeArchived ? "inbox" : "other";
    const triageEvents = template.kind === "archive" || template.kind === "trash"
      ? new Map(targetIds.map((threadId) => {
          const event = template.value
            ? buildTriageDispositionEvent({
                threadId,
                action: template.kind,
                context: triageContext,
                session: triageSessionRef.current,
                now: triageNow(),
                batch: targetIds.length > 1,
              })
            : {
                threadId,
                kind: "restore" as const,
                context: triageSessionRef.current?.threadId === threadId
                  ? triageSessionRef.current.context
                  : triageContext,
                action: template.kind,
                batch: targetIds.length > 1,
              } satisfies TriageEvent;
          return [threadId, event] as const;
        }))
      : null;
    const previous = new Map(
      threads.filter((thread) => targetIds.includes(thread.id)).map((thread) => [thread.id, thread] as const),
    );
    const markRemovedRead = template.value
      && ((template.kind === "archive" && markReadOnArchive) || (template.kind === "trash" && markReadOnTrash));
    const applyChange = (thread: Thread) => {
      const changed = applyMutationTemplate(thread, template);
      return markRemovedRead ? { ...changed, unread: false } : changed;
    };
    const unreadBefore = new Map([...previous].map(([id, thread]) => [id, thread.unread]));
    const previousDetail = detail && targetIds.includes(detail.thread.id) ? detail : null;
    // The list rows captured above belong to this view; they are only put
    // back while it is still the one on screen.
    const mutationView = viewKey;
    const stillInView = () => viewKeyRef.current === mutationView;
    // Custom splits use the Inbox scope. Starred follows the star across
    // reading and archiving until the user unstars, trashes, or spams it.
    const starredView = mailbox === "split" && activeSplitInboxId === STARRED_INBOX_ID && !queryRef.current.trim();
    const removesFromView = starredView
      ? (template.kind === "star" && !template.value) || ((template.kind === "trash" || template.kind === "spam") && template.value)
      : mailbox === "snoozed"
      ? template.kind === "archive" || template.kind === "trash" || template.kind === "spam"
      : mailbox === "trash"
      ? template.kind === "trash" && !template.value
      : mailbox === "allMail" || mailbox === "sent"
        ? template.kind === "trash" && template.value
        // An all-mail search keeps archived results but, like the backend,
        // never shows Trash or Spam.
        : template.value
            && (template.kind === "trash" || template.kind === "spam"
              || (template.kind === "archive" && !includeArchived));

    setThreads((current) => {
      const mapped = current.map((thread) =>
        previous.has(thread.id) ? applyChange(thread) : thread,
      );
      return removesFromView ? mapped.filter((thread) => !targetIds.includes(thread.id)) : mapped;
    });
    setDetail((current) => {
      if (!current || !targetIds.includes(current.thread.id)) return current;
      const messages = template.kind === "read" || markRemovedRead
        ? current.messages.map((message, index) => ({
            ...message,
            unread: markRemovedRead || template.value ? false : index === current.messages.length - 1,
          }))
        : current.messages;
      return { ...current, thread: applyChange(current.thread), messages };
    });

    if (removesFromView && selectedId && targetIds.includes(selectedId)) {
      const currentIndex = threads.findIndex((thread) => thread.id === selectedId);
      const remaining = threads.filter((thread) => !targetIds.includes(thread.id));
      // The list runs newest first: the conversation that slides into the
      // removed row is the next older one, and the row above is newer. At
      // either end, fall back to the neighbor that exists.
      const nextIndex = afterTriage === "newer"
        ? (currentIndex > 0 ? currentIndex - 1 : 0)
        : Math.min(currentIndex, remaining.length - 1);
      setSelectedId(afterTriage === "list" ? null : remaining[nextIndex]?.id ?? null);
    }

    if (removesFromView) {
      setCheckedIds((current) => {
        if (current.size === 0) return current;
        const next = new Set(current);
        targetIds.forEach((id) => next.delete(id));
        return next.size === current.size ? current : next;
      });
    }

    let failedIds: string[] = [];
    let snoozesBefore: ConversationSnooze[] = [];
    try {
      const changesLocation = template.kind === "archive" || template.kind === "trash" || template.kind === "spam"
        || (template.kind === "label" && ["INBOX", "TRASH", "SPAM"].includes(template.labelId));
      if (changesLocation && [...previous.values()].some((thread) => thread.snoozedUntil)) {
        snoozesBefore = (await mailClient.listSnoozeRecords()).filter((record) => targetIds.includes(record.id) && record.state === "active");
      }
      if (markRemovedRead && options?.includeUnloaded) {
        await Promise.all(targetIds.filter((id) => !previous.has(id)).map(async (id) => {
          const snapshot = await mailClient.getThread(id);
          unreadBefore.set(id, snapshot.thread.unread);
        }));
      }
      await mailClient.mutateThreads(targetIds.flatMap((id) => [
        ...(markRemovedRead ? [buildThreadMutation(id, { kind: "read", value: true })] : []),
        buildThreadMutation(id, template),
      ]));
    } catch {
      failedIds = targetIds;
    }
    refreshMailboxUnreadCounts();
    const succeededIds = targetIds.filter((id) => !failedIds.includes(id));

    if (triageEvents) {
      succeededIds.forEach((id) => {
        const event = triageEvents.get(id);
        if (event) recordTriageEvent(event);
      });
    }

    if (failedIds.length > 0 && stillInView()) {
      setThreads((current) => {
        const restored = failedIds
          .map((id) => previous.get(id))
          .filter((thread): thread is Thread => Boolean(thread));
        return sortByRecency([...current.filter((thread) => !failedIds.includes(thread.id)), ...restored]);
      });
    }
    if (failedIds.length > 0 && previousDetail && failedIds.includes(previousDetail.thread.id)) setDetail(previousDetail);

    // Through the ref: the user may have switched mailbox or account during
    // the round trip, and a reload must paint the view now on screen.
    if (removesFromView) {
      await loadThreadsRef.current(queryRef.current);
    }
    if (document.visibilityState !== "visible" || !document.hasFocus()) {
      void mailClient.flushPending().then(setSyncStatus).catch(logBackgroundFailure("Pending mutation flush"));
    }

    if (succeededIds.length === 0) {
      setNotice({ message: "Change could not be saved" });
      return {};
    }

    const labelName = template.kind === "label" ? template.labelName : undefined;
    const definite = describeMutation(template, succeededIds.length, labelName);
    const singleToggle = (template.kind === "star" || template.kind === "read")
      && succeededIds.length === 1 && failedIds.length === 0;
    const message = singleToggle
      ? undefined
      : failedIds.length > 0
        ? `${definite} — ${failedIds.length} could not be saved`
        : definite;
    const undoTemplate = invertMutationTemplate(template);

    return {
      message,
      undoKind: template.kind === "archive" && template.value ? "archive" : undefined,
      undoAction: async () => {
        // Undo can come long after the change, from another mailbox or
        // account. The server-side undo always runs, but the optimistic
        // restore and reselection only apply to the view it was made in.
        if (stillInView()) {
          setThreads((current) => {
            const restored = succeededIds
              .map((id) => previous.get(id))
              .filter((thread): thread is Thread => Boolean(thread));
            return sortByRecency([...current.filter((thread) => !succeededIds.includes(thread.id)), ...restored]);
          });
          if (removesFromView && succeededIds.length === 1) setSelectedId(succeededIds[0]);
        }
        await mailClient.mutateThreads(succeededIds.flatMap((id) => [
          buildThreadMutation(id, undoTemplate),
          ...(markRemovedRead && unreadBefore.get(id)
            ? [buildThreadMutation(id, { kind: "read", value: false })] : []),
        ]));
        const restore = snoozesBefore.filter((record) => succeededIds.includes(record.id));
        if (restore.length) await mailClient.restoreSnoozes(restore, [...previous.values()].filter((thread) => thread.archived).map((thread) => thread.id));
        if (triageEvents) {
          succeededIds.forEach((id) => {
            const event = triageEvents.get(id);
            if (!event || event.kind !== "disposition") return;
            recordTriageEvent({
              threadId: id,
              kind: "restore",
              context: event.context,
              action: event.action,
              batch: event.batch,
            });
          });
        }
        await loadThreadsRef.current(queryRef.current);
      },
    };
  }, [
    selectedId, mailbox, activeSplitInboxId, includeArchived, threads, detail, viewKey, setThreads, setDetail,
    triageSessionRef, setSelectedId, setCheckedIds, recordTriageEvent, loadThreadsRef, queryRef,
    setSyncStatus, setNotice, refreshMailboxUnreadCounts, markReadOnArchive, markReadOnTrash, afterTriage,
  ]);

  const selected = threads.find((thread) => thread.id === selectedId);
  const mutateIdsRef = useRef(mutateIds);
  mutateIdsRef.current = mutateIds;

  // Explicitly marking the open thread unread suppresses auto-read until the
  // selection changes. The timer itself is not recorded here, so changing the
  // configured delay can cancel and reschedule it with the new duration.
  useEffect(() => {
    autoReadSuppressedForId.current = null;
  }, [selectedId]);

  useEffect(() => {
    if (
      !markReadOnOpen
      || !selectedId
      || visibleDetail?.thread.id !== selectedId
      || !isThreadMailbox
      || autoReadSuppressedForId.current === selectedId
    ) return;
    if (!selected?.unread) return;

    const timer = window.setTimeout(() => {
      void mutateIdsRef.current([selectedId], { kind: "read", value: true });
    }, autoReadDelaySeconds * 1000);
    return () => window.clearTimeout(timer);
  }, [autoReadDelaySeconds, isThreadMailbox, markReadOnOpen, selected?.unread, selectedId, visibleDetail?.thread.id]);

  return mutateIds;
}
