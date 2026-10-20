import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Draft } from "./correspondence";
import { ThreadDrafts, threadDrafts } from "./ThreadDrafts";

afterEach(cleanup);

const draft = (overrides: Partial<Draft>): Draft => ({
  id: "d", revision: 1, account: "you@example.com", mode: "reply", sourceId: null, threadId: "t-1", replyId: "m@example.com",
  references: [], to: "jane@example.com", cc: "", bcc: "", subject: "Re: Plans", body: "", attachments: [], updatedAt: 0, ...overrides,
});

describe("threadDrafts", () => {
  it("matches drafts by account and conversation, not by subject", () => {
    const thread = { accountId: "you@example.com", providerThreadId: "t-1" };
    const mine = draft({ id: "mine" });
    const otherAccount = draft({ id: "other-account", account: "work@example.com" });
    const otherThread = draft({ id: "other-thread", threadId: "t-2" });
    const unthreaded = draft({ id: "new", mode: "new", threadId: null });
    expect(threadDrafts([mine, otherAccount, otherThread, unthreaded], thread).map((d) => d.id)).toEqual(["mine"]);
  });
});

describe("ThreadDrafts", () => {
  it("shows each unsent draft as a draft and resumes it", () => {
    const onOpen = vi.fn();
    const reply = draft({});
    render(<ThreadDrafts drafts={[reply, draft({ id: "empty", to: "" })]} onOpen={onOpen} />);
    const cards = screen.getAllByRole("group", { name: "Unsent draft" });
    expect(cards[0]).toHaveTextContent("Draft to jane@example.com");
    expect(cards[1]).toHaveTextContent("Draft with no recipients");
    fireEvent.click(screen.getAllByRole("button", { name: "Resume Draft" })[0]);
    expect(onOpen).toHaveBeenCalledWith(reply);
  });

  it("renders nothing for a conversation without drafts", () => {
    const { container } = render(<ThreadDrafts drafts={[]} onOpen={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });
});
