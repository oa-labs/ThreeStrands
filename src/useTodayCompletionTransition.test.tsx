import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { useRef, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DoneCard, DONE_FILTER_KEY, type DoneEntry } from "./DoneCard";
import { PlanningInbox, type InboxEntry } from "./PlanningInbox";
import { TodayCompletionCheckbox, TODAY_COMPLETION_ACK_MS, TODAY_COMPLETION_DESTINATION_WAIT_MS, TODAY_COMPLETION_HIGHLIGHT_MS, TODAY_COMPLETION_MOVE_MS, useTodayCompletionTransition } from "./useTodayCompletionTransition";

const rect = (top: number, width = 300, height = 40) => ({ x: 20, y: top, left: 20, top, right: 20 + width, bottom: top + height, width, height, toJSON() {} });
const save = vi.fn<() => Promise<string | null>>();
const animations: { element: HTMLElement; frames: Keyframe[]; options: KeyframeAnimationOptions; cancel: ReturnType<typeof vi.fn> }[] = [];
let reduced = false;
let clipped = false;
let offscreen = false;
let sourceVisible = true;
let mediaChange: (() => void) | undefined;

function Harness({ scope = "account:today", today = true, initialDone = false, completionDelay = 0 }: { scope?: string; today?: boolean; initialDone?: boolean; completionDelay?: number }) {
  const page = useRef<HTMLElement>(null);
  const transition = useTodayCompletionTransition(page, scope, today);
  const [done, setDone] = useState(initialDone);
  const complete = async () => {
    const key = await save();
    if (key) {
      if (completionDelay) window.setTimeout(() => setDone(true), completionDelay);
      else setDone(true);
    }
    return key;
  };
  const entries: InboxEntry[] = [
    ...(!done ? [{ id: "first", identities: ["task:first"], category: "tasks" as const, title: "Review proposal", source: null, actions: [], row: {
      control: <TodayCompletionCheckbox label="Complete proposal" checked={false} transition={transition} save={complete} />,
    } }] : []),
    { id: "next", identities: ["task:next"], category: "tasks", title: "Next task", source: null, actions: [], row: {} },
  ];
  const finished: DoneEntry[] = done ? [{ id: "first", identities: ["plan:original", "task:first"], category: "tasks", title: "Review proposal", at: "2026-11-18T10:00:00Z", row: {
    control: <input type="checkbox" aria-label="Reopen proposal" checked onChange={() => setDone(false)} />,
  } }] : [];
  return <section ref={page}>
    <PlanningInbox entries={entries} status={() => null} loading={() => false} hasMore={() => false} onLoadMore={() => {}} drag={{ startDrag() {}, clickSuppressed: () => false, draggingId: null, enabled: false }} />
    <DoneCard title={today ? "Done Today" : "Done"} today={today} entries={finished} errors={{}} loading={false} />
  </section>;
}

const heading = () => screen.getByRole("region", { name: "Done Today" }).querySelector("header")!;
const destination = () => screen.getByRole("checkbox", { name: "Reopen proposal" }).closest<HTMLElement>(".context-row")!;
const ghosts = () => document.querySelectorAll<HTMLElement>(".today-completion-ghost");
async function advance(ms: number) { await act(async () => { vi.advanceTimersByTime(ms); }); }
async function clickComplete() { await act(async () => { fireEvent.click(screen.getByRole("checkbox", { name: "Complete proposal" })); }); }

beforeEach(() => {
  vi.useFakeTimers(); localStorage.clear();
  reduced = false; clipped = false; offscreen = false; sourceVisible = true; animations.length = 0;
  save.mockReset().mockResolvedValue("task:first");
  vi.spyOn(window, "matchMedia").mockImplementation((media) => ({ matches: reduced, media, onchange: null,
    addEventListener: (_event: string, listener: EventListenerOrEventListenerObject) => { mediaChange = listener as () => void; }, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false,
  }));
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    if (this.classList.contains("today-inbox-card")) {
      if (!sourceVisible) return rect(-50);
      return rect(this.dataset.rowKey === "task:next" && screen.queryByRole("checkbox", { name: "Complete proposal" }) ? 160 : 100);
    }
    if (this.classList.contains("today-done-items")) return rect(350, 340, clipped ? 10 : 200);
    if (this.closest(".today-done") && this.classList.contains("context-row")) return rect(offscreen ? window.innerHeight + 50 : 400);
    if (this.classList.contains("today-panel-heading")) return rect(300);
    return rect(0, 800, 700);
  });
  const computedStyle = window.getComputedStyle.bind(window);
  vi.spyOn(window, "getComputedStyle").mockImplementation((element) => {
    const style = computedStyle(element);
    if (element.classList.contains("today-done-items")) style.overflowX = style.overflowY = "auto";
    return style;
  });
  Object.defineProperty(HTMLElement.prototype, "animate", { configurable: true, writable: true, value: function (this: HTMLElement, frames: Keyframe[], options: KeyframeAnimationOptions) {
    const cancel = vi.fn(); animations.push({ element: this, frames, options, cancel }); return { cancel };
  } });
});
afterEach(() => {
  cleanup(); vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals();
  delete (HTMLElement.prototype as Partial<HTMLElement>).animate;
});

describe("Today completion transition", () => {
  it("saves immediately, acknowledges the checked title, carries it to the matching Done row, and smoothly closes the gap", async () => {
    render(<Harness />);
    const checkbox = screen.getByRole("checkbox", { name: "Complete proposal" }); checkbox.focus();
    await clickComplete();
    expect(save).toHaveBeenCalledOnce();
    expect(destination()).toBeInTheDocument();
    expect(ghosts()).toHaveLength(1);
    expect(ghosts()[0]).toHaveAttribute("aria-hidden", "true");
    expect(ghosts()[0].inert).toBe(true);
    expect(ghosts()[0]).toHaveTextContent("Review proposal");
    expect(ghosts()[0].querySelector("input")).toBeChecked();
    expect(ghosts()[0].querySelector("[id], [data-row-key]")).toBeNull();
    expect(screen.getAllByRole("checkbox")).toHaveLength(1);
    expect(within(screen.getByRole("region", { name: "Planning Inbox" })).getByRole("tab", { selected: true })).toHaveFocus();
    await advance(20);
    const flight = animations.find((animation) => animation.element.classList.contains("today-completion-ghost"))!;
    expect(flight.options.duration).toBe(TODAY_COMPLETION_MOVE_MS);
    expect(flight.options.delay).toBeGreaterThan(0);
    expect(flight.options.delay).toBeLessThanOrEqual(TODAY_COMPLETION_ACK_MS);
    expect(flight.frames.at(-1)).toMatchObject({ transform: "translate(0px, 300px)", opacity: 0 });
    expect(animations.find((animation) => animation.element.dataset.rowKey === "task:next")?.frames[0]).toEqual({ transform: "translateY(60px)" });
    expect(animations.find((animation) => animation.element === destination())?.frames[0]).toEqual({ opacity: 0 });
    await advance(TODAY_COMPLETION_ACK_MS + TODAY_COMPLETION_MOVE_MS);
    expect(ghosts()).toHaveLength(0);
    expect(destination()).toHaveClass("today-completion-arrival");
    await advance(TODAY_COMPLETION_HIGHLIGHT_MS);
    expect(destination()).not.toHaveClass("today-completion-arrival");
    expect(animations.every((animation) => animation.cancel.mock.calls.length === 1)).toBe(true);
  });

  it("keeps a pending checkbox checked and prevents duplicate writes, then restores it after a failed save", async () => {
    let finish!: (key: string | null) => void;
    save.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    render(<Harness />);
    const checkbox = screen.getByRole("checkbox", { name: "Complete proposal" }); checkbox.focus();
    await clickComplete();
    expect(checkbox).toBeChecked(); expect(checkbox).toBeDisabled();
    fireEvent.click(checkbox);
    expect(save).toHaveBeenCalledOnce();
    expect(ghosts()).toHaveLength(0);
    expect(animations).toHaveLength(0);
    checkbox.blur();
    await act(async () => finish(null));
    expect(checkbox).toHaveFocus();
    expect(checkbox).not.toBeChecked(); expect(checkbox).toBeEnabled();
    await advance(1000);
    expect(ghosts()).toHaveLength(0); expect(animations).toHaveLength(0);
    expect(heading()).not.toHaveClass("today-completion-arrival");
  });

  it.each(["filtered", "scrolled", "offscreen"])("highlights the heading and count when the destination is %s, preserving the filter and scroll", async (reason) => {
    if (reason === "filtered") localStorage.setItem(DONE_FILTER_KEY, "emails");
    clipped = reason === "scrolled"; offscreen = reason === "offscreen";
    render(<Harness />);
    const panel = within(screen.getByRole("region", { name: "Done Today" })).getByRole("tabpanel"); panel.scrollTop = 75;
    await clickComplete(); await advance(TODAY_COMPLETION_ACK_MS + 30);
    expect(ghosts()).toHaveLength(0);
    expect(animations.some((animation) => animation.element.classList.contains("today-completion-ghost"))).toBe(false);
    expect(heading()).toHaveClass("today-completion-arrival");
    expect(within(heading()).getByLabelText("1 finished item")).toHaveTextContent("1");
    expect(panel.scrollTop).toBe(75);
    expect(within(screen.getByRole("region", { name: "Done Today" })).getByRole("tab", { selected: true })).toHaveAccessibleName(reason === "filtered" ? "Emails" : "All");
  });

  it("waits for saved note-task history to arrive before animating to the row", async () => {
    render(<Harness completionDelay={150} />); await clickComplete(); await advance(20);
    expect(ghosts()).toHaveLength(1); expect(animations).toHaveLength(0);
    await advance(150); await advance(20);
    expect(animations.some((animation) => animation.element === destination())).toBe(true);
    await advance(TODAY_COMPLETION_MOVE_MS);
    expect(ghosts()).toHaveLength(0); expect(destination()).toHaveClass("today-completion-arrival");
  });

  it("falls back to the heading if history has not arrived within a short bounded wait", async () => {
    render(<Harness completionDelay={2000} />); await clickComplete(); await advance(20);
    await advance(TODAY_COMPLETION_DESTINATION_WAIT_MS + 30);
    expect(ghosts()).toHaveLength(0); expect(heading()).toHaveClass("today-completion-arrival");
    expect(animations).toHaveLength(0);
  });

  it("uses an immediate highlight without spatial animation when reduced motion is requested", async () => {
    reduced = true;
    render(<Harness />); await clickComplete(); await advance(20);
    expect(ghosts()).toHaveLength(0); expect(animations).toHaveLength(0);
    expect(destination()).toHaveClass("today-completion-arrival");
    await advance(TODAY_COMPLETION_HIGHLIGHT_MS);
    expect(destination()).not.toHaveClass("today-completion-arrival");
  });

  it("uses the highlight if Web Animations are unavailable", async () => {
    delete (HTMLElement.prototype as Partial<HTMLElement>).animate;
    render(<Harness />); await clickComplete(); await advance(20);
    expect(ghosts()).toHaveLength(0); expect(destination()).toHaveClass("today-completion-arrival");
  });

  it("does not animate existing completions, background refreshes, or reopening a task", async () => {
    const view = render(<Harness initialDone />); await advance(1000);
    view.rerender(<Harness initialDone />);
    fireEvent.click(screen.getByRole("checkbox", { name: "Reopen proposal" })); await advance(1000);
    expect(screen.getByRole("checkbox", { name: "Complete proposal" })).not.toBeChecked();
    expect(save).not.toHaveBeenCalled(); expect(ghosts()).toHaveLength(0); expect(animations).toHaveLength(0);
    expect(document.querySelector(".today-completion-arrival")).toBeNull();
  });

  it("does not transfer today's completion into another day's Done card", async () => {
    render(<Harness today={false} />); await clickComplete(); await advance(1000);
    expect(save).toHaveBeenCalledOnce(); expect(destination()).toBeInTheDocument();
    expect(ghosts()).toHaveLength(0); expect(animations).toHaveLength(0);
    expect(document.querySelector(".today-completion-arrival")).toBeNull();
  });

  it.each(["scroll", "resize", "reduced motion", "filter", "scope", "unmount"])("cleans up a transfer on %s without undoing its save", async (reason) => {
    const view = render(<Harness />); await clickComplete(); await advance(TODAY_COMPLETION_ACK_MS + 30);
    expect(ghosts()).toHaveLength(1);
    if (reason === "filter") fireEvent.click(within(screen.getByRole("region", { name: "Done Today" })).getByRole("tab", { name: "Emails" }));
    else if (reason === "scope") view.rerender(<Harness scope="other-account:today" />);
    else if (reason === "unmount") view.unmount();
    else if (reason === "reduced motion") act(() => mediaChange?.());
    else fireEvent(window, new Event(reason));
    expect(ghosts()).toHaveLength(0);
    expect(animations.every((animation) => animation.cancel.mock.calls.length === 1)).toBe(true);
    await advance(1000);
    expect(document.querySelector(".today-completion-arrival")).toBeNull(); expect(save).toHaveBeenCalledOnce();
  });

  it("does not leave decoration behind when a save finishes after unmount", async () => {
    let finish!: (key: string | null) => void;
    save.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    const view = render(<Harness />); await clickComplete(); view.unmount();
    await act(async () => finish("task:first")); await advance(1000);
    expect(ghosts()).toHaveLength(0); expect(animations).toHaveLength(0);
  });

  it("does not fly a copy from an offscreen source", async () => {
    sourceVisible = false;
    render(<Harness />); await clickComplete(); await advance(20);
    expect(ghosts()).toHaveLength(0); expect(animations).toHaveLength(0);
    expect(destination()).toHaveClass("today-completion-arrival");
  });
});
