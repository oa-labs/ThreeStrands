import type { KeyboardEvent as ReactKeyboardEvent } from "react";

const FOCUSABLE = 'button:not(:disabled), input:not(:disabled), textarea:not(:disabled), select:not(:disabled), a[href], [tabindex="0"]';

/** The panel's controls in tab order, leaving out collapsed sections. */
export function panelFocusables(panel: HTMLElement): HTMLElement[] {
  return [...panel.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((element) => !element.closest("[hidden]"));
}

/**
 * Moves focus into the context panel at its most useful control: the first
 * fix in "Before you send", then Find Times, then whatever comes first. A
 * panel with no controls takes focus itself so it can be scrolled.
 */
export function focusContextPanel(panel: HTMLElement): void {
  const target = panel.querySelector<HTMLElement>(".compose-checks .compose-check button:not(:disabled)")
    ?? panel.querySelector<HTMLElement>(".compose-availability button:not(:disabled)")
    ?? panelFocusables(panel)[0]
    ?? panel;
  target.focus();
}

/**
 * Keys inside the context panel while a draft is open: Escape returns to the
 * draft instead of closing it, and Tab stays within the panel, as it does in
 * the composer. Events from portals rendered by the panel's children (such as
 * dialogs) are left alone.
 */
export function handleContextPanelKeyDown(event: ReactKeyboardEvent<HTMLElement>, returnToDraft: () => void): void {
  const panel = event.currentTarget;
  if (event.nativeEvent.isComposing || !(event.target instanceof Node) || !panel.contains(event.target)) return;
  if (event.key === "Escape") {
    // The composer closes on Escape through a window listener that skips handled events.
    event.preventDefault();
    event.stopPropagation();
    returnToDraft();
    return;
  }
  if (event.key !== "Tab") return;
  const controls = panelFocusables(panel);
  const first = controls[0];
  const last = controls.at(-1);
  if (!first || !last) { event.preventDefault(); return; }
  const active = document.activeElement;
  if (event.shiftKey && (active === first || active === panel)) { event.preventDefault(); last.focus(); }
  else if (!event.shiftKey && (active === last || active === panel)) { event.preventDefault(); first.focus(); }
}
