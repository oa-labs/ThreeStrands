import { useEffect, type RefObject } from "react";
import { logBackgroundFailure } from "./errors";

const PULL_DISTANCE = 80;
const WHEEL_IDLE_MS = 250;
const WHEEL_LINE_HEIGHT = 16;

/** A deliberate downward pull at the top of the list checks the server once. */
export function usePullToRefresh(targetRef: RefObject<HTMLElement | null>, refresh?: () => Promise<void>) {
  useEffect(() => {
    const target = targetRef.current;
    if (!target || !refresh) return;
    let pending = false;
    let distance = 0;
    let locked = false;
    let timer: number | undefined;
    let touch: { id: number; x: number; y: number; distance: number } | null = null;
    const run = () => {
      if (pending) return;
      pending = true;
      void refresh().catch(logBackgroundFailure("Pull to refresh")).finally(() => { pending = false; });
    };
    const onWheel = (event: WheelEvent) => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => { distance = 0; locked = false; }, WHEEL_IDLE_MS);
      // A gesture which starts within the list must finish before a new pull
      // can refresh. Its momentum reaching the top is ordinary scrolling.
      if (target.scrollTop > 0 || event.ctrlKey || event.deltaY >= 0 || Math.abs(event.deltaX) >= Math.abs(event.deltaY)) {
        distance = 0;
        locked = true;
        return;
      }
      if (locked) return;
      const scale = event.deltaMode === WheelEvent.DOM_DELTA_LINE ? WHEEL_LINE_HEIGHT
        : event.deltaMode === WheelEvent.DOM_DELTA_PAGE ? target.clientHeight : 1;
      distance -= event.deltaY * scale;
      if (distance < PULL_DISTANCE) return;
      locked = true;
      run();
    };
    const onTouchStart = (event: TouchEvent) => {
      touch = event.touches.length === 1 && target.scrollTop <= 0
        ? { id: event.touches[0].identifier, x: event.touches[0].clientX, y: event.touches[0].clientY, distance: 0 }
        : null;
    };
    const onTouchMove = (event: TouchEvent) => {
      if (!touch) return;
      const point = Array.from(event.touches).find((point) => point.identifier === touch!.id);
      if (event.touches.length !== 1 || !point || target.scrollTop > 0) { touch = null; return; }
      const dy = point.clientY - touch.y;
      if (dy < 0 || Math.abs(point.clientX - touch.x) > dy) { touch = null; return; }
      touch.distance = dy;
      // Keep a downward pull at the boundary from becoming native overscroll.
      if (event.cancelable) event.preventDefault();
    };
    const onTouchEnd = () => {
      if (touch && touch.distance >= PULL_DISTANCE) run();
      touch = null;
    };
    const onTouchCancel = () => { touch = null; };
    target.addEventListener("wheel", onWheel, { passive: true });
    target.addEventListener("touchstart", onTouchStart, { passive: true });
    target.addEventListener("touchmove", onTouchMove, { passive: false });
    target.addEventListener("touchend", onTouchEnd);
    target.addEventListener("touchcancel", onTouchCancel);
    return () => {
      window.clearTimeout(timer);
      target.removeEventListener("wheel", onWheel);
      target.removeEventListener("touchstart", onTouchStart);
      target.removeEventListener("touchmove", onTouchMove);
      target.removeEventListener("touchend", onTouchEnd);
      target.removeEventListener("touchcancel", onTouchCancel);
    };
  }, [targetRef, refresh]);
}
