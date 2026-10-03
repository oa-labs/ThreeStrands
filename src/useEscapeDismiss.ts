import { useLayoutEffect, useRef } from "react";

type DismissEntry = {
  id: symbol;
  dismiss: () => void;
};

const dismissStack: DismissEntry[] = [];

function handleEscape(event: KeyboardEvent) {
  if (event.key !== "Escape" || event.isComposing || event.defaultPrevented) return;
  const topmost = dismissStack.at(-1);
  if (!topmost) return;

  event.preventDefault();
  event.stopImmediatePropagation();
  topmost.dismiss();
}

export function useEscapeDismiss(onDismiss: () => void, enabled = true) {
  const dismiss = useRef(onDismiss);
  dismiss.current = onDismiss;

  // Register during commit, not after paint: once an overlay is in the DOM,
  // Escape must reach it even if passive effects have not flushed yet.
  useLayoutEffect(() => {
    if (!enabled) return;
    const entry: DismissEntry = {
      id: Symbol("escape-dismiss"),
      dismiss: () => dismiss.current(),
    };
    dismissStack.push(entry);
    if (dismissStack.length === 1) window.addEventListener("keydown", handleEscape);

    return () => {
      const index = dismissStack.findIndex((candidate) => candidate.id === entry.id);
      if (index !== -1) dismissStack.splice(index, 1);
      if (dismissStack.length === 0) window.removeEventListener("keydown", handleEscape);
    };
  }, [enabled]);
}
