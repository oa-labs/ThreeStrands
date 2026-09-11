import { useEffect, useRef, useState } from "react";

const storageKey = "dispatch.inboxWidth";
const minimumWidth = 280;
const defaultWidth = 400;
const maximumWidth = 640;

function readWidth() {
  try {
    const saved = Number(localStorage.getItem(storageKey));
    if (Number.isFinite(saved) && saved >= minimumWidth && saved <= maximumWidth) return saved;
  } catch {
    // Resizing remains available when storage is blocked.
  }
  return defaultWidth;
}

export function useInboxWidth() {
  const [preferredWidth, setPreferredWidth] = useState(readWidth);
  const [viewportWidth, setViewportWidth] = useState(window.innerWidth);
  useEffect(() => {
    const onResize = () => setViewportWidth(window.innerWidth);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  // Keep room for the reader without overwriting the user's preferred width
  // when the app window temporarily becomes smaller.
  const maxWidth = Math.max(minimumWidth, Math.min(maximumWidth, viewportWidth - 58 - 420));
  const width = Math.min(preferredWidth, maxWidth);
  const resize = (next: number) => {
    const clamped = Math.round(Math.max(minimumWidth, Math.min(maxWidth, next)));
    setPreferredWidth(clamped);
    try {
      localStorage.setItem(storageKey, String(clamped));
    } catch {
      // Retain the width for this session if persistence is unavailable.
    }
  };
  return { width, maxWidth, resize };
}

export function InboxResizeHandle({ width, maxWidth, resize }: ReturnType<typeof useInboxWidth>) {
  const drag = useRef<{ x: number; width: number; pointerId: number } | null>(null);
  const [dragging, setDragging] = useState(false);
  return (
    <div
      className={`inbox-resizer${dragging ? " dragging" : ""}`}
      role="separator"
      tabIndex={0}
      aria-label="Resize inbox"
      aria-orientation="vertical"
      aria-controls="inbox-panel"
      aria-valuemin={minimumWidth}
      aria-valuemax={maxWidth}
      aria-valuenow={width}
      aria-valuetext={`${width} pixels`}
      title="Drag to resize inbox. Use arrow keys to adjust; double-click to reset."
      onPointerDown={(event) => {
        if (event.button !== 0) return;
        event.preventDefault();
        event.currentTarget.focus();
        event.currentTarget.setPointerCapture(event.pointerId);
        drag.current = { x: event.clientX, width, pointerId: event.pointerId };
        setDragging(true);
      }}
      onPointerMove={(event) => {
        if (drag.current?.pointerId === event.pointerId) {
          resize(drag.current.width + event.clientX - drag.current.x);
        }
      }}
      onPointerUp={(event) => {
        if (drag.current?.pointerId === event.pointerId) {
          event.currentTarget.releasePointerCapture(event.pointerId);
          drag.current = null;
          setDragging(false);
        }
      }}
      onLostPointerCapture={() => {
        drag.current = null;
        setDragging(false);
      }}
      onDoubleClick={() => resize(defaultWidth)}
      onKeyDown={(event) => {
        const step = event.shiftKey ? 40 : 10;
        const next = event.key === "ArrowLeft" ? width - step
          : event.key === "ArrowRight" ? width + step
          : event.key === "Home" ? minimumWidth
          : event.key === "End" ? maxWidth : null;
        if (next !== null) {
          event.preventDefault();
          event.stopPropagation();
          resize(next);
        }
      }}
    />
  );
}
