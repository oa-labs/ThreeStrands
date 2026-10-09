import { useCallback, useEffect, useRef, useState } from "react";

export type Notice = { message: string; undo?: () => void };

export const NOTICE_TIMEOUT_MS = 6000;

let noticeSequence = 0;

export function useNotice() {
  const [notice, setCurrent] = useState<(Notice & { key: number }) | null>(null);
  const timeout = useRef<number | null>(null);

  const clearTimer = useCallback(() => {
    if (timeout.current === null) return;
    window.clearTimeout(timeout.current);
    timeout.current = null;
  }, []);

  const setNotice = useCallback((next: Notice | null) => {
    clearTimer();
    if (!next) {
      setCurrent(null);
      return;
    }
    const key = ++noticeSequence;
    setCurrent({ ...next, key });
    timeout.current = window.setTimeout(() => {
      timeout.current = null;
      setCurrent((shown) => (shown?.key === key ? null : shown));
    }, NOTICE_TIMEOUT_MS);
  }, [clearTimer]);

  useEffect(() => clearTimer, [clearTimer]);

  return [notice, setNotice] as const;
}
