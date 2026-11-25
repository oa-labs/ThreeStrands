import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { OutboxItem } from "./correspondence";
import { useNotice } from "./useNotice";
import { useSendTaskSuggestion } from "./useSendTaskSuggestion";

const queued = (draft: Partial<OutboxItem["draft"]> = {}) =>
  ({ id: "send-1", state: "ready", draft: { id: "d1", ...draft } }) as OutboxItem;

function mount(onLinked = vi.fn()) {
  const view = renderHook(() => {
    const [notice, setNotice] = useNotice();
    return { notice, setNotice, suggest: useSendTaskSuggestion(notice, setNotice, onLinked) };
  });
  return { ...view, onLinked };
}

describe("useSendTaskSuggestion", () => {
  beforeEach(() => {
    vi.spyOn(mailClient, "suggestTaskForOutbox").mockResolvedValue({ taskId: "t1", title: "Send the contract" });
    vi.spyOn(mailClient, "linkTaskToOutbox").mockResolvedValue("queued");
  });
  afterEach(() => vi.restoreAllMocks());

  it("offers the matching task after a send", async () => {
    const { result } = mount();
    await act(async () => result.current.suggest(queued()));
    expect(mailClient.suggestTaskForOutbox).toHaveBeenCalledWith("send-1");
    expect(result.current.notice?.message).toBe("Mark “Send the contract” done?");
    expect(result.current.notice?.action?.label).toBe("Mark Done");
  });

  it("shows nothing when nothing matches or the lookup fails", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const { result } = mount();
    vi.mocked(mailClient.suggestTaskForOutbox).mockResolvedValueOnce(null);
    await act(async () => result.current.suggest(queued()));
    expect(result.current.notice).toBeNull();
    vi.mocked(mailClient.suggestTaskForOutbox).mockRejectedValueOnce(new Error("broken"));
    await act(async () => result.current.suggest(queued()));
    expect(result.current.notice).toBeNull();
  });

  it("does not look for a match when the draft already has a link", async () => {
    const { result } = mount();
    await act(async () => result.current.suggest(queued({ completesTaskId: "t9" })));
    await act(async () => result.current.suggest(queued({ followUpTaskId: "t8" })));
    expect(mailClient.suggestTaskForOutbox).not.toHaveBeenCalled();
    expect(result.current.notice).toBeNull();
  });

  it("links the queued message on Mark Done and says it will finish when sent", async () => {
    const { result, onLinked } = mount();
    await act(async () => result.current.suggest(queued()));
    await act(async () => result.current.notice?.action?.run());
    expect(mailClient.linkTaskToOutbox).toHaveBeenCalledExactlyOnceWith("send-1", "t1");
    expect(onLinked).toHaveBeenCalledWith("queued");
    expect(result.current.notice?.message).toBe("“Send the contract” will be marked done when it’s sent");
  });

  it("says the task is done when the message had already been sent", async () => {
    vi.mocked(mailClient.linkTaskToOutbox).mockResolvedValue("sent");
    const { result, onLinked } = mount();
    await act(async () => result.current.suggest(queued()));
    await act(async () => result.current.notice?.action?.run());
    expect(onLinked).toHaveBeenCalledWith("sent");
    expect(result.current.notice?.message).toBe("Marked “Send the contract” done");
  });

  it("reports a message that wasn't sent", async () => {
    vi.mocked(mailClient.linkTaskToOutbox).mockRejectedValue(new Error("That message wasn't sent"));
    const { result, onLinked } = mount();
    await act(async () => result.current.suggest(queued()));
    await act(async () => result.current.notice?.action?.run());
    expect(onLinked).not.toHaveBeenCalled();
    expect(result.current.notice?.message).toContain("That message wasn't sent");
  });

  it("does not replace an Undo notice that is showing", async () => {
    const { result } = mount();
    act(() => result.current.setNotice({ message: "Archived", undo: vi.fn() }));
    await act(async () => result.current.suggest(queued()));
    expect(result.current.notice?.message).toBe("Archived");
  });

  it("does not replace a notice the send itself showed meanwhile", async () => {
    let answer: (value: { taskId: string; title: string }) => void = () => {};
    vi.mocked(mailClient.suggestTaskForOutbox).mockReturnValue(new Promise((resolve) => { answer = resolve; }));
    const { result } = mount();
    await act(async () => result.current.suggest(queued()));
    act(() => result.current.setNotice({ message: "Sent and archived" }));
    await act(async () => answer({ taskId: "t1", title: "Send the contract" }));
    expect(result.current.notice?.message).toBe("Sent and archived");
  });
});
