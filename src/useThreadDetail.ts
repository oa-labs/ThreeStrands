import { useEffect, useRef, useState } from "react";
import type { Thread, ThreadDetail } from "./domain";
import { mailClient } from "./data/client";
import { errorMessage } from "./errors";
import type { Notice } from "./useNotice";

export function useThreadDetail(selectedId: string | null, threads: Thread[], setNotice: (notice: Notice | null) => void) {
  const [detail, setDetail] = useState<ThreadDetail | null>(null);
  const visibleDetail = detail?.thread.id === selectedId ? detail : null;
  const detailRequest = useRef(0);
  const [detailLoading, setDetailLoading] = useState(false);
  const selectedThread = threads.find((thread) => thread.id === selectedId);
  const selectedThreadLastMessageAt = selectedThread?.lastMessageAt;
  const selectedThreadSnippet = selectedThread?.snippet;
  useEffect(() => {
    const requestId = ++detailRequest.current;
    if (!selectedId) {
      setDetail(null);
      setDetailLoading(false);
      return;
    }
    // Keep the existing conversation visible while refreshing it after a
    // send. Clear immediately only when the user selected a different thread.
    setDetail((current) => current?.thread.id === selectedId ? current : null);
    setDetailLoading(true);
    void mailClient.getThread(selectedId)
      .then((next) => {
        if (requestId !== detailRequest.current || next.thread.id !== selectedId) return;
        setDetail(next);
      })
      .catch((error) => {
        if (requestId !== detailRequest.current) return;
        setDetail(null);
        setNotice({ message: `Could not open conversation: ${errorMessage(error)}` });
      })
      .finally(() => {
        if (requestId === detailRequest.current) setDetailLoading(false);
      });
  }, [selectedId, selectedThreadLastMessageAt, selectedThreadSnippet, setNotice]);

  return { detail, setDetail, visibleDetail, detailLoading, setDetailLoading };
}
