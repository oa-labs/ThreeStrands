import { act, cleanup, renderHook } from "@testing-library/react";
import { useMemo, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { OutboxItem } from "./correspondence";
import { mailClient } from "./data/client";
import type { ThreadDetail } from "./domain";
import { messagesWithQueuedReplies } from "./queuedReplies";
import { SENT_REPLY_REFRESH_DELAYS_MS, useSentReplyRefresh } from "./useSentReplyRefresh";

function sentReply(detail: ThreadDetail, state: OutboxItem["state"] = "sent"): OutboxItem {
  return {
    id: "reply-all",
    state,
    deadline: Date.now(),
    error: null,
    providerId: state === "sent" ? "provider-sent-reply" : null,
    draft: {
      id: "reply-all-draft",
      revision: 1,
      account: "demo@example.com",
      mode: "replyAll",
      sourceId: detail.messages.at(-1)!.id,
      threadId: detail.thread.providerThreadId,
      replyId: null,
      references: [],
      to: "jane@example.com, brian@example.com",
      cc: "",
      bcc: "",
      subject: detail.thread.subject,
      body: "Thanks, everyone",
      bodyHtml: "<p>Thanks, everyone</p>",
      attachments: [],
      updatedAt: Date.now(),
    },
  };
}

function withProviderCopy(detail: ThreadDetail): ThreadDetail {
  return {
    ...detail,
    messages: [...detail.messages, {
      ...detail.messages.at(-1)!,
      id: "provider-sent-reply",
      sender: "demo@example.com",
      bodyText: "Thanks, everyone",
      bodyHtml: "<p>Thanks, everyone</p>",
      sentAt: new Date().toISOString(),
    }],
  };
}

describe("useSentReplyRefresh", () => {
  let stale: ThreadDetail;

  beforeEach(async () => {
    stale = await mailClient.getThread("welcome");
    vi.useFakeTimers();
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  function setup(outbox: OutboxItem[]) {
    return renderHook(() => {
      const [detail, setDetail] = useState<ThreadDetail | null>(stale);
      const displayed = useMemo(() => detail ? messagesWithQueuedReplies(detail, outbox) : [], [detail]);
      useSentReplyRefresh(detail, displayed, outbox, setDetail);
      return displayed;
    });
  }

  it("re-reads the conversation until a sent reply's provider copy replaces its outbox copy", async () => {
    const getThread = vi.spyOn(mailClient, "getThread")
      .mockResolvedValueOnce(stale)
      .mockResolvedValue(withProviderCopy(stale));
    const { result } = setup([sentReply(stale)]);
    expect(result.current.at(-1)!.id).toBe("outbox-reply-all");

    await act(async () => { await vi.advanceTimersByTimeAsync(SENT_REPLY_REFRESH_DELAYS_MS[0]!); });
    expect(getThread).toHaveBeenCalledTimes(1);
    expect(result.current.at(-1)!.id).toBe("outbox-reply-all");

    await act(async () => { await vi.advanceTimersByTimeAsync(SENT_REPLY_REFRESH_DELAYS_MS[1]!); });
    expect(getThread).toHaveBeenCalledTimes(2);
    expect(result.current.at(-1)!.id).toBe("provider-sent-reply");

    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(getThread).toHaveBeenCalledTimes(2);
  });

  it("keeps checking at the capped interval when the provider copy arrives after the initial retries", async () => {
    const getThread = vi.spyOn(mailClient, "getThread").mockResolvedValue(stale);
    const { result, unmount } = setup([sentReply(stale)]);
    const total = SENT_REPLY_REFRESH_DELAYS_MS.reduce((sum, delay) => sum + delay, 0);
    await act(async () => { await vi.advanceTimersByTimeAsync(total + 60_000); });
    expect(getThread).toHaveBeenCalledTimes(SENT_REPLY_REFRESH_DELAYS_MS.length + 2);
    expect(result.current.at(-1)!.id).toBe("outbox-reply-all");

    getThread.mockResolvedValue(withProviderCopy(stale));
    await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
    expect(result.current.at(-1)!.id).toBe("provider-sent-reply");
    const calls = getThread.mock.calls.length;
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(getThread).toHaveBeenCalledTimes(calls);
    unmount();
  });

  it("stops checking when the conversation closes", async () => {
    const getThread = vi.spyOn(mailClient, "getThread").mockResolvedValue(stale);
    const { unmount } = setup([sentReply(stale)]);
    await act(async () => { await vi.advanceTimersByTimeAsync(SENT_REPLY_REFRESH_DELAYS_MS[0]!); });
    expect(getThread).toHaveBeenCalledTimes(1);
    unmount();
    await act(async () => { await vi.advanceTimersByTimeAsync(120_000); });
    expect(getThread).toHaveBeenCalledTimes(1);
  });

  it("leaves a reply still waiting to send to the outbox refresh", async () => {
    const getThread = vi.spyOn(mailClient, "getThread");
    const { result } = setup([sentReply(stale, "undo_pending")]);
    expect(result.current.at(-1)!.id).toBe("outbox-reply-all");

    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(getThread).not.toHaveBeenCalled();
  });
});
