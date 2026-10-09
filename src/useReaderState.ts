import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { Message, ThreadDetail } from "./domain";

/**
 * Owns state that belongs to the conversation reader rather than the mailbox
 * or application shell. Expansion is intentionally keyed by message id so
 * unread mutations cannot unexpectedly collapse cards while they are open.
 */
export function useReaderState({
  selectedThreadId,
  detail,
  displayedMessages,
  composerOpen,
}: {
  selectedThreadId: string | null;
  detail: ThreadDetail | null;
  displayedMessages: Message[];
  composerOpen: boolean;
}) {
  const [messageExpansionOverrides, setMessageExpansionOverrides] = useState<Map<string, boolean>>(new Map());
  const latestMessageRef = useRef<HTMLElement | null>(null);
  const messageStackRef = useRef<HTMLDivElement>(null);
  const messageRefs = useRef<Map<string, HTMLElement>>(new Map());
  const activeMessageIdRef = useRef<string | null>(null);
  const pendingMessageToggleFocusRef = useRef<string | null>(null);
  const [activeMessageId, setActiveMessageId] = useState<string | null>(null);
  const lastAutoScrollRef = useRef<{ threadId: string; messageId: string } | null>(null);

  useEffect(() => {
    setMessageExpansionOverrides(new Map());
    messageRefs.current.clear();
    activeMessageIdRef.current = null;
    pendingMessageToggleFocusRef.current = null;
    setActiveMessageId(null);
    lastAutoScrollRef.current = null;
  }, [selectedThreadId]);

  useEffect(() => {
    if (!detail) return;
    setMessageExpansionOverrides((current) => {
      const next = new Map<string, boolean>();
      for (const [index, message] of detail.messages.entries()) {
        const isLatest = index === detail.messages.length - 1;
        next.set(message.id, current.get(message.id) ?? (isLatest || message.unread));
      }
      return next;
    });
  }, [detail]);

  const latestDisplayedMessageId = displayedMessages.at(-1)?.id;
  const detailThreadId = detail?.thread.id;
  useEffect(() => {
    if (!latestDisplayedMessageId) return;
    activeMessageIdRef.current = latestDisplayedMessageId;
    setActiveMessageId(latestDisplayedMessageId);
  }, [detailThreadId, latestDisplayedMessageId]);

  useEffect(() => {
    if (!detailThreadId || !latestDisplayedMessageId || composerOpen) return;
    // Read status, summaries, and other metadata replace detail without
    // opening a conversation or adding a message. Leave the reader in place.
    const previous = lastAutoScrollRef.current;
    if (previous?.threadId === detailThreadId && previous.messageId === latestDisplayedMessageId) return;
    lastAutoScrollRef.current = { threadId: detailThreadId, messageId: latestDisplayedMessageId };
    latestMessageRef.current?.scrollIntoView?.({ block: "start" });
  }, [composerOpen, detailThreadId, latestDisplayedMessageId]);

  useLayoutEffect(() => {
    // Collapsing tall cards can leave Chrome's scroll offset beyond the new
    // content height. Re-clamp it after layout so the pane cannot go blank.
    const node = messageStackRef.current;
    if (!node) return;
    const maxScrollTop = Math.max(0, node.scrollHeight - node.clientHeight);
    if (node.scrollTop > maxScrollTop) node.scrollTop = maxScrollTop;
  }, [messageExpansionOverrides]);

  useLayoutEffect(() => {
    const messageId = pendingMessageToggleFocusRef.current;
    if (!messageId) return;
    pendingMessageToggleFocusRef.current = null;
    const node = messageRefs.current.get(messageId);
    const toggle = node?.querySelector<HTMLElement>(".message-card-toggle, .message-expanded-toggle");
    toggle?.focus({ preventScroll: true });
  }, [messageExpansionOverrides]);

  return {
    messageExpansionOverrides,
    setMessageExpansionOverrides,
    latestMessageRef,
    messageStackRef,
    messageRefs,
    activeMessageIdRef,
    pendingMessageToggleFocusRef,
    activeMessageId,
    setActiveMessageId,
    latestDisplayedMessageId,
  };
}
