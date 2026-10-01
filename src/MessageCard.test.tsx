import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Message } from "./domain";
import * as inlineAttachments from "./inlineAttachments";
import { MessageCard } from "./MessageCard";

vi.mock("./inlineAttachments", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./inlineAttachments")>();
  return { ...actual, referencedImageContentIds: vi.fn(actual.referencedImageContentIds) };
});

const message: Message = {
  id: "m1",
  threadId: "t1",
  sender: "Ada Lovelace <ada@example.com>",
  recipients: ["me@example.com"],
  sentAt: "2026-09-01T10:00:00Z",
  bodyHtml: "<p>Hello there</p>",
  bodyText: "Hello there",
  unread: false,
  attachments: [],
};

const stableProps = {
  index: 0,
  isLatest: true,
  isActive: false,
  accounts: [],
  queuedItem: undefined,
  loadRemoteImages: false,
  theme: "dark" as const,
  fontScale: 1,
  fontFamily: "system" as const,
  onActivate: vi.fn(),
  onToggle: vi.fn(),
  onRespond: vi.fn(),
  onRegisterNode: vi.fn(),
  onImageClick: vi.fn(),
  onNotice: vi.fn(),
};

describe("MessageCard", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("skips re-parsing its message when the parent re-renders for unrelated state", () => {
    let bump: () => void = () => {};
    function Parent() {
      const [count, setCount] = useState(0);
      bump = () => setCount((value) => value + 1);
      return <><span data-testid="count">{count}</span><MessageCard message={message} isExpanded {...stableProps} /></>;
    }
    render(<Parent />);
    const frame = screen.getByTestId("message-body");
    const parses = vi.mocked(inlineAttachments.referencedImageContentIds).mock.calls.length;
    expect(parses).toBeGreaterThan(0);

    act(() => bump());
    act(() => bump());

    expect(screen.getByTestId("count")).toHaveTextContent("2");
    expect(vi.mocked(inlineAttachments.referencedImageContentIds).mock.calls.length).toBe(parses);
    expect(screen.getByTestId("message-body")).toBe(frame);
  });

  it("reports toggles and responses by message id", () => {
    render(<MessageCard message={message} isExpanded {...stableProps} />);
    fireEvent.click(screen.getByRole("button", { name: "Reply All" }));
    fireEvent.click(screen.getByRole("button", { name: /Collapse message from/ }));

    expect(stableProps.onRespond).toHaveBeenCalledWith("replyAll", "m1");
    expect(stableProps.onToggle).toHaveBeenCalledWith("m1", true);
  });

  it("renders a collapsed card with a snippet and no message frame", () => {
    render(<MessageCard message={{ ...message, bodyText: "Hello&nbsp;there   friend" }} isExpanded={false} {...stableProps} />);
    expect(screen.queryByTestId("message-body")).not.toBeInTheDocument();
    expect(screen.getByText("Hello there friend")).toBeInTheDocument();
  });
});
