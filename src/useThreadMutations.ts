import { useCallback, useEffect, useRef, type Dispatch, type SetStateAction } from "react";
import type { CommandResult, MailboxKind } from "./commands";
import { mailClient } from "./data/client";
import type { SyncStatus, Thread, ThreadDetail, TriageEvent } from "./domain";
import { logBackgroundFailure } from "./errors";
import {
  applyMutationTemplate,
  buildThreadMutation,
  describeMutation,
  invertMutationTemplate,
  type MutationTemplate,
} from "./threadMutations";
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
  mailbox: MailboxKind;
  activeAccountId: string | null;
  activeSplitInboxId: string | null;
  includeArchived: boolean;
  isThreadMailbox: boolean;
  autoReadDelaySeconds: number;
};

/** Applies optimistic mail changes, rollback and undo within the originating view. */
export function useThreadMutations({
  threads, detail, visibleDetail, selectedId, setSelectedId, setDetail, setThreads, setCheckedIds,
  setSyncStatus, setNotice, mailbox, activeAccountId, activeSplitInboxId, includeArchived,
  isThreadMailbox, autoReadDelaySeconds, triageSessionRef, recordTriageEvent, loadThreadsRef,
  queryRef,
}: Options) {
  const autoReadSuppressedForId = useRef<string | null>(null);
  // Identifies the thread list on screen, so work that outlives a view switch
  // (a mutation's round trip, a later Undo) can tell whether to touch it.
  const viewKey = [mailbox, activeAccountId ?? "", activeSplitInboxId ?? "", includeArchived].join("\u0000");
  const viewKeyRef = useRef(viewKey);
  viewKeyRef.current = viewKey;

  const mutateIds = useCallback(async (ids: string[], template: MutationTemplate): Promise<CommandResult> => {
    const targetIds = ids.filter((id) => threads.some((thread) => thread.id === id));
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
    const previousDetail = detail && targetIds.includes(detail.thread.id) ? detail : null;
    // The list rows captured above belong to this view; they are only put
    // back while it is still the one on screen.
    const mutationView = viewKey;
    const stillInView = () => viewKeyRef.current === mutationView;
    // "split" intentionally falls into the default branch below: a split
    // inbox's underlying set is the same unarchived/untrashed inbox scope,
    // so archiving/trashing/spamming a thread should remove it exactly like
    // it would from the plain Inbox.
    const removesFromView = mailbox === "trash"
      ? template.kind === "trash" && !template.value
      : mailbox === "allMail"
        ? template.kind === "trash" && template.value
        : (template.kind === "archive" || template.kind === "trash" || template.kind === "spam")
            && template.value
            && !includeArchived;

    setThreads((current) => {
      const mapped = current.map((thread) =>
        previous.has(thread.id) ? applyMutationTemplate(thread, template) : thread,
      );
      return removesFromView ? mapped.filter((thread) => !targetIds.includes(thread.id)) : mapped;
    });
    setDetail((current) => {
      if (!current || !targetIds.includes(current.thread.id)) return current;
      const messages = template.kind === "read"
        ? current.messages.map((message, index) => ({
            ...message,
            unread: template.value ? false : index === current.messages.length - 1,
          }))
        : current.messages;
      return { ...current, thread: applyMutationTemplate(current.thread, template), messages };
    });

    if (removesFromView && selectedId && targetIds.includes(selectedId)) {
      const currentIndex = threads.findIndex((thread) => thread.id === selectedId);
      const remaining = threads.filter((thread) => !targetIds.includes(thread.id));
      const nextIndex = Math.min(currentIndex, remaining.length - 1);
      setSelectedId(remaining[nextIndex]?.id ?? null);
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
    try {
      await mailClient.mutateThreads(targetIds.map((id) => buildThreadMutation(id, template)));
    } catch {
      failedIds = targetIds;
    }
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
        await mailClient.mutateThreads(succeededIds.map((id) => buildThreadMutation(id, undoTemplate)));
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
    selectedId, mailbox, includeArchived, threads, detail, viewKey, setThreads, setDetail,
    triageSessionRef, setSelectedId, setCheckedIds, recordTriageEvent, loadThreadsRef, queryRef,
    setSyncStatus, setNotice,
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
      !selectedId
      || visibleDetail?.thread.id !== selectedId
      || !isThreadMailbox
      || autoReadSuppressedForId.current === selectedId
    ) return;
    if (!selected?.unread) return;

    const timer = window.setTimeout(() => {
      void mutateIdsRef.current([selectedId], { kind: "read", value: true });
    }, autoReadDelaySeconds * 1000);
    return () => window.clearTimeout(timer);
  }, [autoReadDelaySeconds, isThreadMailbox, selected?.unread, selectedId, visibleDetail?.thread.id]);

  return mutateIds;
}
