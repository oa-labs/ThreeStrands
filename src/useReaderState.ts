import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { Message, ThreadDetail } from "./domain";
import { messageAtReadingLine } from "./readingMessage";

/** Input that means the reader is moving through the conversation themselves. */
const RELEASE_EVENTS = ["wheel", "touchmove", "pointerdown", "keydown"] as const;

/** Layout position in the document, ignoring scroll and transforms such as a card's entrance animation. */
function layoutTop(node: HTMLElement) {
  let top = 0;
  for (let current: HTMLElement | null = node; current; current = current.offsetParent as HTMLElement | null) top += current.offsetTop;
  return top;
}

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
  // The message being read, for the context panel. It follows a message the
  // reader clicks or moves to, and otherwise whichever message is at the
  // reading line as the reader scrolls. A clicked message holds while any of
  // it is on screen, so scrolling a little does not move the panel away.
  const readingMessageIdRef = useRef<string | null>(null);
  const [readingMessageId, setReadingMessageIdState] = useState<string | null>(null);
  const pinnedReadingRef = useRef<string | null>(null);
  const setReadingMessage = useCallback((messageId: string | null, pinned = false) => {
    pinnedReadingRef.current = pinned ? messageId : null;
    if (readingMessageIdRef.current === messageId) return;
    readingMessageIdRef.current = messageId;
    setReadingMessageIdState(messageId);
  }, []);
  const lastAutoScrollRef = useRef<{ threadId: string; messageId: string } | null>(null);

  useEffect(() => {
    setMessageExpansionOverrides(new Map());
    messageRefs.current.clear();
    activeMessageIdRef.current = null;
    pendingMessageToggleFocusRef.current = null;
    setActiveMessageId(null);
    setReadingMessage(null);
    lastAutoScrollRef.current = null;
  }, [selectedThreadId, setReadingMessage]);

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

  const displayedMessagesRef = useRef(displayedMessages);
  displayedMessagesRef.current = displayedMessages;
  const latestDisplayedMessageId = displayedMessages.at(-1)?.id;
  const detailThreadId = detail?.thread.id;
  useEffect(() => {
    if (!latestDisplayedMessageId) return;
    activeMessageIdRef.current = latestDisplayedMessageId;
    setActiveMessageId(latestDisplayedMessageId);
    setReadingMessage(latestDisplayedMessageId);
  }, [detailThreadId, latestDisplayedMessageId, setReadingMessage]);

  useEffect(() => {
    const stack = messageStackRef.current;
    if (!stack || !detailThreadId) return;
    let frame = 0;
    const update = () => {
      frame = 0;
      const bounds = stack.getBoundingClientRect();
      const pinned = pinnedReadingRef.current ? messageRefs.current.get(pinnedReadingRef.current) : null;
      if (pinned) {
        const rect = pinned.getBoundingClientRect();
        if (rect.bottom > bounds.top && rect.top < bounds.bottom) return;
      }
      const messages = displayedMessagesRef.current.flatMap((message) => {
        const node = messageRefs.current.get(message.id);
        return node ? [{ id: message.id, top: node.getBoundingClientRect().top }] : [];
      });
      const reading = messageAtReadingLine({
        top: bounds.top,
        height: stack.clientHeight,
        atBottom: stack.scrollTop + stack.clientHeight >= stack.scrollHeight - 2,
      }, messages);
      if (reading) setReadingMessage(reading);
    };
    const onScroll = () => { if (!frame) frame = requestAnimationFrame(update); };
    stack.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      stack.removeEventListener("scroll", onScroll);
      if (frame) cancelAnimationFrame(frame);
    };
  }, [detailThreadId, setReadingMessage]);

  useEffect(() => {
    if (!detailThreadId || !latestDisplayedMessageId || composerOpen) return;
    // Read status, summaries, and other metadata replace detail without
    // opening a conversation or adding a message. Leave the reader in place.
    const previous = lastAutoScrollRef.current;
    if (previous?.threadId === detailThreadId && previous.messageId === latestDisplayedMessageId) return;
    lastAutoScrollRef.current = { threadId: detailThreadId, messageId: latestDisplayedMessageId };
    const latest = latestMessageRef.current;
    latest?.scrollIntoView?.({ block: "start" });
    // The email body sizes itself after this first scroll. Until then the card
    // is short, so the stack can only scroll far enough to leave it at the
    // bottom. Keep it at the top as it grows, until the reader takes over.
    // Input inside the email's iframe never reaches this window, so a scroll
    // that moves the stack away from where the pin left it also releases it.
    const stack = messageStackRef.current;
    if (!latest || !stack || typeof ResizeObserver === "undefined") return;
    let pinnedScrollTop = stack.scrollTop;
    // Scroll only the stack: scrollIntoView would also nudge any scrollable
    // ancestor, such as the window, every time the body grows.
    const observer = new ResizeObserver(() => {
      const offset = layoutTop(latest) - layoutTop(stack);
      stack.scrollTop = offset - (parseFloat(getComputedStyle(stack).scrollPaddingTop) || 0);
      pinnedScrollTop = stack.scrollTop;
    });
    observer.observe(latest);
    const onScroll = () => { if (Math.abs(stack.scrollTop - pinnedScrollTop) > 1) release(); };
    const release = () => {
      observer.disconnect();
      stack.removeEventListener("scroll", onScroll);
      for (const type of RELEASE_EVENTS) window.removeEventListener(type, release, true);
    };
    stack.addEventListener("scroll", onScroll, { passive: true });
    for (const type of RELEASE_EVENTS) window.addEventListener(type, release, { capture: true, passive: true });
    return release;
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
    readingMessageId,
    readingMessageIdRef,
    setReadingMessage,
    latestDisplayedMessageId,
  };
}
