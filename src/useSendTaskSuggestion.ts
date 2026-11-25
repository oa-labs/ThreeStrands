import { useCallback, useRef } from "react";
import { mailClient } from "./data/client";
import type { LinkedSend } from "./domain";
import type { OutboxItem } from "./correspondence";
import { errorMessage, logBackgroundFailure } from "./errors";
import type { Notice } from "./useNotice";

type ShownNotice = (Notice & { key: number }) | null;

/**
 * After **Send** or **Schedule** queues a message with no task link, offers to
 * mark the one task it probably finishes done. Nothing completes without the
 * click: **Mark Done** links the queued message, and the task finishes when it
 * leaves (or now, if it already has).
 *
 * The answer arrives after the send's own notice (Send & Archive's Undo, say),
 * so it shows only when the notice slot is as it was when the message was
 * queued and holds no Undo. A newer notice is never replaced.
 */
export function useSendTaskSuggestion(
  notice: ShownNotice,
  setNotice: (next: Notice | null) => void,
  onLinked: (result: LinkedSend) => void,
) {
  const shown = useRef(notice);
  shown.current = notice;
  const link = useRef(onLinked);
  link.current = onLinked;

  return useCallback((item: OutboxItem) => {
    if (item.draft.completesTaskId || item.draft.followUpTaskId) return;
    const before = shown.current?.key ?? null;
    void mailClient.suggestTaskForOutbox(item.id).then((suggestion) => {
      if (!suggestion) return;
      const current = shown.current;
      if (current?.undo || (current?.key ?? null) !== before) return;
      const { taskId, title } = suggestion;
      setNotice({
        message: `Mark “${title}” done?`,
        action: {
          label: "Mark Done",
          run: () => void mailClient.linkTaskToOutbox(item.id, taskId).then((result) => {
            link.current(result);
            setNotice({ message: result === "sent" ? `Marked “${title}” done` : `“${title}” will be marked done when it’s sent` });
          }, (reason: unknown) => setNotice({ message: errorMessage(reason) })),
        },
      });
    }, logBackgroundFailure("Task suggestion after sending"));
  }, [setNotice]);
}
