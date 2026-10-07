import type { MailboxKind } from "./commands";

/** Workspaces that can cover the mail reader, as App tracks them. */
export type ReturnWorkspace = "calendar" | "contacts" | "tasks" | "week" | null;

/** Where the reader was scrolled: a message and how far past its top edge. */
export type ReaderAnchor = { messageId: string; offset: number };

/** Everything needed to put the user back where a jump started. */
export type ReturnPoint = {
  accountId: string | null;
  mailbox: MailboxKind;
  splitInboxId: string | null;
  query: string;
  searchOpen: boolean;
  workspace: ReturnWorkspace;
  threadId: string | null;
  /** Names the place for the Back link, such as a subject or "Tasks". */
  label: string;
  /** The open conversation's expanded messages and scroll position. */
  reader: { expanded: [string, boolean][]; anchor: ReaderAnchor | null } | null;
};

/** One jump: where it started, and the conversation it opened. */
export type ReturnStep = { origin: ReturnPoint; target: string };

/** Oldest steps drop off past this, so a long chain of jumps cannot grow without bound. */
export const RETURN_STEP_LIMIT = 20;

/**
 * Records a jump. The steps form a stack so that a later back/forward history
 * can build on them; for now only the newest step is offered.
 */
export function pushReturnStep(steps: readonly ReturnStep[], step: ReturnStep): ReturnStep[] {
  if (step.origin.threadId === step.target && step.origin.workspace === null) return [...steps];
  return [...steps, step].slice(-RETURN_STEP_LIMIT);
}

/** The step Back would undo, while the user is still on the conversation it opened. */
export function currentReturnStep(steps: readonly ReturnStep[], selectedId: string | null): ReturnStep | null {
  const top = steps.at(-1);
  return top && top.target === selectedId ? top : null;
}

/**
 * Forgets every step once the user goes somewhere themselves: a step only
 * makes sense from the conversation it opened.
 */
export function settleReturnSteps(steps: readonly ReturnStep[], selectedId: string | null): readonly ReturnStep[] {
  return steps.length === 0 || currentReturnStep(steps, selectedId) ? steps : [];
}

/** The first message showing at the top of the scrolled reader, and how far it is scrolled past. */
export function captureReaderAnchor(stack: HTMLElement | null, messages: ReadonlyMap<string, HTMLElement>): ReaderAnchor | null {
  if (!stack) return null;
  const top = stack.getBoundingClientRect().top;
  // Registration order is not guaranteed to be screen order, so compare positions.
  let anchor: { messageId: string; rectTop: number } | null = null;
  for (const [messageId, node] of messages) {
    const rect = node.getBoundingClientRect();
    if (rect.bottom > top && (!anchor || rect.top < anchor.rectTop)) anchor = { messageId, rectTop: rect.top };
  }
  return anchor ? { messageId: anchor.messageId, offset: top - anchor.rectTop } : null;
}

/** Scrolls the reader so the anchored message sits where it was; false if the message is gone. */
export function restoreReaderAnchor(stack: HTMLElement | null, messages: ReadonlyMap<string, HTMLElement>, anchor: ReaderAnchor): boolean {
  const node = messages.get(anchor.messageId);
  if (!stack || !node) return false;
  stack.scrollTop += node.getBoundingClientRect().top - stack.getBoundingClientRect().top + anchor.offset;
  return true;
}
