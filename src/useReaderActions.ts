import { useCallback, useRef } from "react";
import type { Message, ThreadDetail, Thread } from "./domain";
import type { MessageResponseKind } from "./MessageCard";
import type { MailboxKind } from "./commands";
import type { useReaderState } from "./useReaderState";
import type { useCorrespondence } from "./useCorrespondence";
import type { useTriageSession } from "./useTriageSession";

export function scrollBehavior(): ScrollBehavior {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth";
}

type Options = Pick<ReturnType<typeof useTriageSession>, "recordTriageEvent"> & {
  reader: Pick<ReturnType<typeof useReaderState>, "activeMessageIdRef" | "setActiveMessageId" | "pendingMessageToggleFocusRef" | "setMessageExpansionOverrides" | "messageRefs" | "latestMessageRef">;
  displayedMessages: Message[];
  visibleDetail: ThreadDetail | null;
  selected: Thread | null;
  correspondence: Pick<ReturnType<typeof useCorrespondence>, "context">;
  jumpToThread: (threadId: string) => void;
  mailbox: MailboxKind;
  includeArchived: boolean;
};

export function useReaderActions({
  reader, displayedMessages, visibleDetail, selected, correspondence, jumpToThread, mailbox,
  includeArchived, recordTriageEvent,
}: Options) {
  const {
    activeMessageIdRef, setActiveMessageId, pendingMessageToggleFocusRef,
    setMessageExpansionOverrides, messageRefs, latestMessageRef,
  } = reader;
  const selectAdjacentMessage = useCallback((direction: -1 | 1) => {
    if (displayedMessages.length === 0) return;
    const activeIndex = displayedMessages.findIndex((message) => message.id === activeMessageIdRef.current);
    const currentIndex = activeIndex >= 0 ? activeIndex : displayedMessages.length - 1;
    const targetIndex = Math.max(0, Math.min(displayedMessages.length - 1, currentIndex + direction));
    const target = displayedMessages[targetIndex];
    if (!target) return;

    activeMessageIdRef.current = target.id;
    const node = messageRefs.current.get(target.id);
    if (!node) return;
    const focusTarget = node.querySelector<HTMLElement>(".message-card-toggle, .message-expanded-toggle") ?? node;
    focusTarget.focus({ preventScroll: true });
    node.scrollIntoView?.({ block: "nearest", behavior: scrollBehavior() });
  }, [activeMessageIdRef, displayedMessages, messageRefs]);

  // Stable callbacks for MessageCard, so a card re-renders only when its own
  // message or display state changes rather than on every App render.
  const activateMessage = useCallback((messageId: string) => {
    activeMessageIdRef.current = messageId;
    setActiveMessageId(messageId);
  }, [activeMessageIdRef, setActiveMessageId]);
  const toggleMessage = useCallback((messageId: string, isExpanded: boolean) => {
    activateMessage(messageId);
    pendingMessageToggleFocusRef.current = messageId;
    setMessageExpansionOverrides((current) => {
      const next = new Map(current);
      next.set(messageId, !isExpanded);
      return next;
    });
  }, [activateMessage, pendingMessageToggleFocusRef, setMessageExpansionOverrides]);
  // Context panel links reveal a message: expanded, active, and scrolled into
  // view when it is in the open conversation, otherwise by opening its thread.
  const showMessage = useCallback((threadId: string, messageId: string) => {
    if (threadId !== visibleDetail?.thread.id) {
      jumpToThread(threadId);
      return;
    }
    activateMessage(messageId);
    setMessageExpansionOverrides((current) => new Map(current).set(messageId, true));
    requestAnimationFrame(() => {
      messageRefs.current.get(messageId)?.scrollIntoView?.({ block: "start", behavior: scrollBehavior() });
    });
  }, [activateMessage, jumpToThread, messageRefs, setMessageExpansionOverrides, visibleDetail?.thread.id]);
  const registerMessageNode = useCallback((messageId: string, isLatest: boolean, node: HTMLElement | null) => {
    if (isLatest) latestMessageRef.current = node;
    if (node) messageRefs.current.set(messageId, node);
    else messageRefs.current.delete(messageId);
  }, [latestMessageRef, messageRefs]);
  const respondToMessageRef = useRef<(kind: MessageResponseKind, messageId: string) => void>(() => {});
  respondToMessageRef.current = (kind, messageId) => {
    if (selected) {
      recordTriageEvent({
        threadId: selected.id,
        kind: "response",
        context: mailbox === "inbox" && !includeArchived ? "inbox" : "other",
      });
    }
    correspondence.context[kind](messageId);
  };
  const respondToMessage = useCallback((kind: MessageResponseKind, messageId: string) => {
    respondToMessageRef.current(kind, messageId);
  }, []);
  return { selectAdjacentMessage, activateMessage, toggleMessage, showMessage, registerMessageNode, respondToMessage };
}
