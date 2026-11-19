import { useEffect, useRef, useState, type RefObject } from "react";

/** Direct manipulation feedback is longer than the app's short surface entrances. */
export const TODAY_COMPLETION_ACK_MS = 100;
export const TODAY_COMPLETION_MOVE_MS = 280;
export const TODAY_COMPLETION_HIGHLIGHT_MS = 600;
/** A note write can finish before the refreshed Done list arrives. */
export const TODAY_COMPLETION_DESTINATION_WAIT_MS = 500;

type SaveCompletion = () => Promise<string | null>;
export type TodayCompletionTransition = (input: HTMLInputElement, save: SaveCompletion) => Promise<string | null>;

/** Visible inside the viewport and every scrolling/clipping ancestor, without scrolling anything into view. */
function visible(element: HTMLElement): boolean {
  const bounds = element.getBoundingClientRect();
  if (!bounds.width || !bounds.height || element.closest("[hidden]")) return false;
  let left = 0, top = 0, right = window.innerWidth, bottom = window.innerHeight;
  for (let parent = element.parentElement; parent; parent = parent.parentElement) {
    const style = getComputedStyle(parent);
    const rect = parent.getBoundingClientRect();
    if (/(auto|scroll|hidden|clip)/.test(style.overflowX || style.overflow)) { left = Math.max(left, rect.left); right = Math.min(right, rect.right); }
    if (/(auto|scroll|hidden|clip)/.test(style.overflowY || style.overflow)) { top = Math.max(top, rect.top); bottom = Math.min(bottom, rect.bottom); }
  }
  return bounds.left >= left && bounds.right <= right && bounds.top >= top && bounds.bottom <= bottom;
}

/** Only an explicit, successfully saved checkbox action can start a transfer. Reloads never do. */
export function useTodayCompletionTransition(page: RefObject<HTMLElement | null>, scope: string, today = true): TodayCompletionTransition {
  const active = useRef(new Set<() => void>());
  const highlighted = useRef(new Map<HTMLElement, number>());
  useEffect(() => {
    const cancel = () => { for (const stop of active.current) stop(); };
    const motion = window.matchMedia("(prefers-reduced-motion: reduce)");
    // A viewport change invalidates the measured path. Cancel decoration, never the write.
    window.addEventListener("scroll", cancel, true);
    window.addEventListener("resize", cancel);
    motion.addEventListener("change", cancel);
    const root = page.current;
    const filterChanged = (event: Event) => {
      if (event.target instanceof Element && event.target.closest('[role="tab"]')) cancel();
    };
    root?.addEventListener("click", filterChanged);
    return () => {
      cancel();
      window.removeEventListener("scroll", cancel, true);
      window.removeEventListener("resize", cancel);
      motion.removeEventListener("change", cancel);
      root?.removeEventListener("click", filterChanged);
    };
  }, [page, scope, today]);

  return async (input, save) => {
    const source = input.closest<HTMLElement>(".today-inbox-card");
    if (!source || !today) return save();
    const started = performance.now();
    const bounds = source.getBoundingClientRect();
    const move = !window.matchMedia("(prefers-reduced-motion: reduce)").matches && typeof source.animate === "function" && visible(source);
    const ghost = move ? source.cloneNode(true) as HTMLElement : null;
    const siblings = move ? [...(source.parentElement?.querySelectorAll<HTMLElement>(".today-inbox-card") ?? [])]
      .filter((row) => row !== source).map((row) => ({ row, top: row.getBoundingClientRect().top })) : [];
    const animations: Animation[] = [];
    const highlights: HTMLElement[] = [];
    const timers: number[] = [];
    let frame = 0;
    let observer: MutationObserver | undefined;
    let destinationTimer = 0;
    let waited = false;
    let stopped = false;
    const stop = () => {
      if (stopped) return;
      stopped = true;
      observer?.disconnect();
      window.clearTimeout(destinationTimer);
      ghost?.remove();
      animations.forEach((animation) => animation.cancel());
      highlights.forEach((element) => {
        const remaining = (highlighted.current.get(element) ?? 1) - 1;
        if (remaining) highlighted.current.set(element, remaining);
        else { highlighted.current.delete(element); element.classList.remove("today-completion-arrival"); }
      });
      timers.forEach(window.clearTimeout);
      window.cancelAnimationFrame(frame);
      active.current.delete(stop);
    };
    active.current.add(stop);
    if (ghost) {
      ghost.classList.add("today-completion-ghost");
      ghost.setAttribute("aria-hidden", "true");
      ghost.inert = true;
      // The copy has no identities, interactive controls, or duplicate accessible content.
      for (const element of [ghost, ...ghost.querySelectorAll<HTMLElement>("*")]) {
        element.removeAttribute("id"); element.removeAttribute("data-row-key");
      }
      ghost.querySelectorAll<HTMLInputElement>('input[type="checkbox"]').forEach((checkbox) => { checkbox.checked = true; });
      Object.assign(ghost.style, { left: `${bounds.left}px`, top: `${bounds.top}px`, width: `${bounds.width}px`, height: `${bounds.height}px` });
    }
    let key: string | null;
    try { key = await save(); }
    catch (reason) { stop(); throw reason; }
    if (!key || stopped) { stop(); return key; }
    // Keep the checked title at its original position for the brief acknowledgement,
    // even if React already removed the saved task from the inbox.
    if (ghost) document.body.append(ghost);
    const arrive = () => {
      window.cancelAnimationFrame(frame);
      frame = window.requestAnimationFrame(() => {
        if (stopped) return;
        const acknowledgement = move ? Math.max(0, TODAY_COMPLETION_ACK_MS - (performance.now() - started)) : 0;
        const done = page.current?.querySelector<HTMLElement>(".today-done");
        const row = [...(done?.querySelectorAll<HTMLElement>("[data-row-key]") ?? [])].find((candidate) => candidate.dataset.rowKey === key);
        const heading = done?.querySelector<HTMLElement>(".today-panel-heading");
        const filter = done?.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')?.dataset.filter;
        if (move && done && !row && !waited && (filter === "all" || filter === "tasks")) {
          // Wait briefly for asynchronous note-task history, without changing the user's filter.
          if (!observer) {
            observer = new MutationObserver(arrive);
            observer.observe(done, { childList: true, subtree: true, attributes: true, attributeFilter: ["aria-selected"] });
            destinationTimer = window.setTimeout(() => { waited = true; observer?.disconnect(); arrive(); }, TODAY_COMPLETION_DESTINATION_WAIT_MS);
          }
          return;
        }
        observer?.disconnect(); window.clearTimeout(destinationTimer);
        const destination = row && visible(row) ? row : heading;
        const highlight = () => {
          if (!destination) { stop(); return; }
          destination.classList.add("today-completion-arrival"); highlights.push(destination);
          highlighted.current.set(destination, (highlighted.current.get(destination) ?? 0) + 1);
          timers.push(window.setTimeout(stop, TODAY_COMPLETION_HIGHLIGHT_MS));
        };
        for (const sibling of siblings) {
          if (!sibling.row.isConnected || !visible(sibling.row)) continue;
          const offset = sibling.top - sibling.row.getBoundingClientRect().top;
          if (offset) animations.push(sibling.row.animate([{ transform: `translateY(${offset}px)` }, { transform: "translateY(0)" }], { duration: TODAY_COMPLETION_MOVE_MS, easing: "ease-out" }));
        }
        if (!ghost || destination !== row || !row || !visible(row)) { ghost?.remove(); highlight(); return; }
        const target = row.getBoundingClientRect();
        animations.push(ghost.animate([
          { transform: "translate(0, 0)", opacity: 1 },
          { opacity: 1, offset: 0.8 },
          { transform: `translate(${target.left - bounds.left}px, ${target.top - bounds.top}px)`, opacity: 0 },
        ], { duration: TODAY_COMPLETION_MOVE_MS, delay: acknowledgement, easing: "cubic-bezier(0.2, 0.8, 0.2, 1)" }));
        animations.push(row.animate([
          { opacity: 0 }, { opacity: 0, offset: (acknowledgement + TODAY_COMPLETION_MOVE_MS * 0.65) / (acknowledgement + TODAY_COMPLETION_MOVE_MS) }, { opacity: 1 },
        ], { duration: acknowledgement + TODAY_COMPLETION_MOVE_MS }));
        timers.push(window.setTimeout(() => { ghost.remove(); highlight(); }, acknowledgement + TODAY_COMPLETION_MOVE_MS));
      });
    };
    arrive();
    return key;
  };
}

/** Immediate checked feedback and duplicate-write protection, shared by all Today completion sources. */
export function TodayCompletionCheckbox({ checked, disabled, label, transition, save }: {
  checked: boolean;
  disabled?: boolean;
  label: string;
  transition: TodayCompletionTransition;
  save: SaveCompletion;
}) {
  const [pending, setPending] = useState(false);
  const writing = useRef(false);
  return <input type="checkbox" className="context-task-checkbox" checked={checked || pending} disabled={disabled || pending}
    aria-label={label} onChange={(event) => {
      if (writing.current) return;
      const input = event.currentTarget;
      const focused = document.activeElement === input;
      writing.current = true; setPending(true);
      void transition(event.currentTarget, save).then((key) => {
        if (!key) {
          writing.current = false; setPending(false);
          if (focused && input.isConnected && document.activeElement === document.body) {
            input.disabled = false; input.focus({ preventScroll: true });
          }
        }
      });
    }} />;
}
