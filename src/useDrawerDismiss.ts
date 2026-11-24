import { useCallback, useEffect, useRef, type KeyboardEvent, type RefObject } from "react";

/**
 * Dismissal for a floating drawer opened by a toggle button: a pointerdown
 * outside it (the toggle excepted, since it toggles itself) or Escape while
 * focus is inside closes it. When focus was inside, it returns to the toggle.
 * Spread `onKeyDown` onto the drawer; call `close` for any other close button.
 */
export function useDrawerDismiss(open: boolean, setOpen: (open: boolean) => void, toggle: RefObject<HTMLElement | null>) {
  const drawer = useRef<HTMLElement | null>(null);
  const close = useCallback(() => {
    const hadFocus = drawer.current?.contains(document.activeElement) ?? false;
    setOpen(false);
    if (hadFocus) toggle.current?.focus();
  }, [setOpen, toggle]);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      const target = event.target as Node;
      if (drawer.current?.contains(target) || toggle.current?.contains(target)) return;
      setOpen(false);
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open, setOpen, toggle]);
  const onKeyDown = useCallback((event: KeyboardEvent) => {
    if (event.key !== "Escape" || event.defaultPrevented || event.nativeEvent.isComposing) return;
    event.preventDefault();
    close();
  }, [close]);
  return { drawer, close, onKeyDown };
}
