import { useEffect, useRef, useState } from "react";

type ResizableWidthConfig = {
  storageKey: string;
  minimumWidth: number;
  defaultWidth: number;
  maximumWidth: number;
  /** Viewport width kept free for the icon rail and the sibling pane when clamping. */
  reservedWidth: number;
};

function readStoredWidth({ storageKey, minimumWidth, defaultWidth, maximumWidth }: ResizableWidthConfig) {
  try {
    const saved = Number(localStorage.getItem(storageKey));
    if (Number.isFinite(saved) && saved >= minimumWidth && saved <= maximumWidth) return saved;
  } catch {
    // Resizing remains available when storage is blocked.
  }
  return defaultWidth;
}

export function useResizableWidth(config: ResizableWidthConfig) {
  const [preferredWidth, setPreferredWidth] = useState(() => readStoredWidth(config));
  const [viewportWidth, setViewportWidth] = useState(window.innerWidth);
  useEffect(() => {
    const onResize = () => setViewportWidth(window.innerWidth);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  const { storageKey } = config;
  useEffect(() => {
    const timer = window.setTimeout(() => {
      try {
        localStorage.setItem(storageKey, String(preferredWidth));
      } catch {
        // Retain the width for this session if persistence is unavailable.
      }
    }, 150);
    return () => window.clearTimeout(timer);
  }, [storageKey, preferredWidth]);
  // Keep room for the sibling pane without overwriting the user's preferred width
  // when the app window temporarily becomes smaller.
  const maxWidth = Math.max(config.minimumWidth, Math.min(config.maximumWidth, viewportWidth - config.reservedWidth));
  const width = Math.min(preferredWidth, maxWidth);
  const resize = (next: number) => {
    const clamped = Math.round(Math.max(config.minimumWidth, Math.min(maxWidth, next)));
    setPreferredWidth(clamped);
  };
  return { width, minWidth: config.minimumWidth, defaultWidth: config.defaultWidth, maxWidth, resize };
}

const inboxWidthConfig: ResizableWidthConfig = {
  storageKey: "threestrands.inboxWidth",
  minimumWidth: 280,
  defaultWidth: 400,
  maximumWidth: 640,
  reservedWidth: 58 + 420,
};

export function useInboxWidth() {
  return useResizableWidth(inboxWidthConfig);
}

// Icon rail plus three board columns at their minimum width.
const taskDetailWidthConfig: ResizableWidthConfig = {
  storageKey: "threestrands.taskDetailWidth",
  minimumWidth: 280,
  defaultWidth: 440,
  maximumWidth: 720,
  reservedWidth: 58 + 626,
};

export function useTaskDetailWidth() {
  return useResizableWidth(taskDetailWidthConfig);
}

export function PanelResizeHandle({
  width,
  minWidth,
  defaultWidth,
  maxWidth,
  resize,
  label,
  controlsId,
  title,
  panelSide = "left",
}: {
  width: number;
  minWidth: number;
  defaultWidth: number;
  maxWidth: number;
  resize: (next: number) => void;
  label: string;
  controlsId: string;
  title: string;
  /** Which side of the handle the resizable panel sits on; drag and arrow-key direction follow it. */
  panelSide?: "left" | "right";
}) {
  const drag = useRef<{ x: number; width: number; pointerId: number } | null>(null);
  const [dragging, setDragging] = useState(false);
  const direction = panelSide === "left" ? 1 : -1;
  return (
    <div
      className={`panel-resizer${dragging ? " dragging" : ""}`}
      role="separator"
      tabIndex={0}
      aria-label={label}
      aria-orientation="vertical"
      aria-controls={controlsId}
      aria-valuemin={minWidth}
      aria-valuemax={maxWidth}
      aria-valuenow={width}
      aria-valuetext={`${width} pixels`}
      title={title}
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
          resize(drag.current.width + direction * (event.clientX - drag.current.x));
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
        const target = event.key === "Home" ? minWidth
          : event.key === "End" ? maxWidth : null;
        if (target !== null) {
          event.preventDefault();
          event.stopPropagation();
          resize(target);
          return;
        }
        const step = event.shiftKey ? 40 : 10;
        const delta = event.key === "ArrowLeft" ? -step
          : event.key === "ArrowRight" ? step : null;
        if (delta !== null) {
          event.preventDefault();
          event.stopPropagation();
          resize(width + direction * delta);
        }
      }}
    />
  );
}
