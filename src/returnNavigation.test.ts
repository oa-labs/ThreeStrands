import { describe, expect, it } from "vitest";
import { captureReaderAnchor, currentReturnStep, pushReturnStep, RETURN_STEP_LIMIT, restoreReaderAnchor, settleReturnSteps, type ReturnPoint, type ReturnStep } from "./returnNavigation";

const point = (threadId: string | null, overrides: Partial<ReturnPoint> = {}): ReturnPoint => ({
  accountId: null, mailbox: "inbox", splitInboxId: null, query: "", searchOpen: false,
  workspace: null, threadId, label: threadId ?? "Inbox", reader: null, ...overrides,
});
const step = (from: string | null, to: string, overrides: Partial<ReturnPoint> = {}): ReturnStep => ({ origin: point(from, overrides), target: to });

/** jsdom does no layout, so each element's box is set by hand. */
function box(element: HTMLElement, top: number, height: number) {
  element.getBoundingClientRect = () => ({ top, bottom: top + height, left: 0, right: 0, width: 0, height, x: 0, y: top, toJSON: () => ({}) });
}

describe("return steps", () => {
  it("offers the newest step only while its conversation is open", () => {
    const steps = pushReturnStep(pushReturnStep([], step("a", "b")), step("b", "c"));
    expect(currentReturnStep(steps, "c")?.origin.threadId).toBe("b");
    expect(currentReturnStep(steps, "b")).toBeNull();
    // Going back pops a step, and the one before it applies again.
    expect(currentReturnStep(steps.slice(0, -1), "b")?.origin.threadId).toBe("a");
  });

  it("forgets every step once the user goes somewhere themselves, and keeps them otherwise", () => {
    const steps = pushReturnStep(pushReturnStep([], step("a", "b")), step("b", "c"));
    expect(settleReturnSteps(steps, "c")).toBe(steps);
    expect(settleReturnSteps(steps, "z")).toEqual([]);
    expect(settleReturnSteps(steps, null)).toEqual([]);
  });

  it("skips a jump to the conversation already open, unless it leaves a workspace", () => {
    expect(pushReturnStep([], step("a", "a"))).toEqual([]);
    expect(pushReturnStep([], step("a", "a", { workspace: "tasks" }))).toHaveLength(1);
  });

  it("keeps at most the limit, dropping the oldest", () => {
    let steps: ReturnStep[] = [];
    for (let index = 0; index <= RETURN_STEP_LIMIT; index++) steps = pushReturnStep(steps, step(`t${index}`, `t${index + 1}`));
    expect(steps).toHaveLength(RETURN_STEP_LIMIT);
    expect(steps[0].origin.threadId).toBe("t1");
    expect(steps.at(-1)?.target).toBe(`t${RETURN_STEP_LIMIT + 1}`);
  });
});

describe("reader anchor", () => {
  it("captures the top visible message by position and scrolls it back to the same offset", () => {
    const stack = document.createElement("div");
    const first = document.createElement("article");
    const second = document.createElement("article");
    box(stack, 100, 500);
    box(first, -300, 350);
    box(second, 50, 400);
    // Registered out of screen order: the anchor still follows the layout.
    const messages = new Map([["second", second], ["first", first]]);
    const anchor = captureReaderAnchor(stack, messages);
    expect(anchor).toEqual({ messageId: "second", offset: 50 });

    // Reopened scrolled to the top, with the message lower down.
    stack.scrollTop = 0;
    box(second, 400, 400);
    expect(restoreReaderAnchor(stack, messages, anchor!)).toBe(true);
    expect(stack.scrollTop).toBe(350);
  });

  it("does nothing without a reader or the anchored message", () => {
    const stack = document.createElement("div");
    expect(captureReaderAnchor(null, new Map())).toBeNull();
    expect(captureReaderAnchor(stack, new Map())).toBeNull();
    expect(restoreReaderAnchor(stack, new Map(), { messageId: "gone", offset: 10 })).toBe(false);
  });
});
