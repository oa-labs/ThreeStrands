import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ContactCardContext, type ContactCardActions } from "./ContactCard";
import { mailClient } from "./data/client";
import type { Account, ContactProfile, Message } from "./domain";
import * as inlineAttachments from "./inlineAttachments";
import { MessageCard, placeHoverCard } from "./MessageCard";
import { threadTextIndex } from "./threadTextIndex";

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

  it("folds a signature repeated from an earlier message in the conversation", () => {
    const earlier: Message = { ...message, id: "m0", bodyHtml: "", bodyText: "First question?\n\nAda Lovelace\nAnalytical Engine Society\nLondon office" };
    const latest: Message = {
      ...message,
      bodyHtml: "<p>Answer.</p><p>Ada Lovelace<br>Analytical Engine Society<br>London office</p><div>On Mon, B wrote:</div><blockquote>First question?</blockquote>",
    };
    const threadText = threadTextIndex([earlier, latest]);
    render(<MessageCard message={latest} isExpanded {...stableProps} index={1} threadText={threadText} />);
    const frame = screen.getByTestId("message-body") as HTMLIFrameElement;
    expect(frame.srcdoc).toContain("Answer.");
    expect(frame.srcdoc).not.toContain("Analytical Engine Society");
  });

  it("renders a collapsed card with a snippet and no message frame", () => {
    render(<MessageCard message={{ ...message, bodyText: "Hello&nbsp;there   friend" }} isExpanded={false} {...stableProps} />);
    expect(screen.queryByTestId("message-body")).not.toBeInTheDocument();
    expect(screen.getByText("Hello there friend")).toBeInTheDocument();
  });

  describe("address contact cards", () => {
    const ada: ContactProfile = {
      id: "contact:ada", displayName: "Ada Lovelace", role: "Analyst", company: "Engines Ltd", location: null, bio: null,
      notes: null, links: [], photoData: null, favorite: false, addresses: ["ada@example.com"], sentCount: 3, receivedCount: 4, lastInteractedAt: null, birthday: null, keepInTouch: { intervalDays: null, startedAt: null, snoozedUntil: null, snoozedAt: null, lastTouchAt: null }, keepInTouchDueAt: null,
    };
    const own = [{ email: "me@example.com" } as Account];
    function renderWithCard(actions: Partial<ContactCardActions> = {}) {
      const value: ContactCardActions = { onOpenContact: vi.fn(), onSelectPerson: vi.fn(), selectedEmail: null, ...actions };
      render(<ContactCardContext.Provider value={value}><MessageCard message={message} isExpanded {...stableProps} accounts={own} /></ContactCardContext.Provider>);
      return value;
    }
    function mockLookup() {
      vi.spyOn(mailClient, "resolveContactIds").mockResolvedValue({ "ada@example.com": ada.id });
      vi.spyOn(mailClient, "getContactProfile").mockResolvedValue(ada);
      vi.spyOn(mailClient, "contactActivity").mockResolvedValue({
        sentCount: 3, receivedCount: 4, threadCount: 2, firstAt: "2026-06-01T00:00:00Z", lastSentAt: null, recentReceivedAt: [],
      });
    }

    it("shows the contact card for a sender on hover, looked up only when opened", async () => {
      mockLookup();
      renderWithCard();
      const sender = screen.getByRole("button", { name: "Ada Lovelace" });
      expect(mailClient.resolveContactIds).not.toHaveBeenCalled();

      fireEvent.mouseEnter(sender.parentElement!);
      const card = screen.getByRole("group", { name: "Contact card for Ada Lovelace" });
      expect(await within(card).findByText("Analyst · Engines Ltd")).toBeInTheDocument();
      expect(within(card).getByText(/7 emails since/)).toBeInTheDocument();
      expect(within(card).getByRole("button", { name: "Copy email address" })).toBeInTheDocument();
      expect(within(card).getByRole("button", { name: "Add favorite" })).toBeInTheDocument();
      // The card names the person without adding a heading to the reader.
      expect(within(card).queryByRole("heading")).not.toBeInTheDocument();

      // The card renders at the top of the page, outside the reader's scroll area.
      expect(card.parentElement).toBe(document.body);
      expect(sender.closest(".message-card")).not.toContainElement(card);

      // Leaving the name gives the pointer a moment to cross into the card.
      fireEvent.mouseLeave(sender.parentElement!);
      fireEvent.mouseEnter(card);
      await new Promise((resolve) => setTimeout(resolve, 200));
      expect(screen.getByRole("group", { name: "Contact card for Ada Lovelace" })).toBeInTheDocument();
      fireEvent.mouseLeave(card);
      await waitFor(() => expect(screen.queryByRole("group", { name: "Contact card for Ada Lovelace" })).not.toBeInTheDocument());
    });

    it("opens the card on keyboard focus and opens the saved contact from it", async () => {
      mockLookup();
      const actions = renderWithCard();
      fireEvent.focus(screen.getByRole("button", { name: "Ada Lovelace" }));
      const card = screen.getByRole("group", { name: "Contact card for Ada Lovelace" });
      fireEvent.click(await within(card).findByRole("button", { name: "Ada Lovelace" }));
      expect(actions.onOpenContact).toHaveBeenCalledWith(ada.id);
    });

    it("makes a clicked name the subject of the context panel and marks it", () => {
      mockLookup();
      const actions = renderWithCard({ selectedEmail: "ada@example.com" });
      const sender = screen.getByRole("button", { name: "Ada Lovelace" });
      expect(sender).toHaveAccessibleDescription("Shows this person in the context panel");
      expect(sender.parentElement).toHaveClass("address-selected");
      fireEvent.click(sender);
      expect(actions.onSelectPerson).toHaveBeenCalledWith("ada@example.com");
      // Selecting stays inside the header instead of toggling the message.
      expect(stableProps.onToggle).not.toHaveBeenCalled();
    });

    it("keeps the plain address popover for the user's own address and without the panel", () => {
      const lookup = vi.spyOn(mailClient, "resolveContactIds");
      renderWithCard();
      expect(screen.getByRole("button", { name: "Copy me@example.com" })).toBeInTheDocument();
      expect(screen.queryByRole("group", { name: /Contact card for me@example.com/ })).not.toBeInTheDocument();
      cleanup();

      render(<MessageCard message={message} isExpanded {...stableProps} accounts={own} />);
      expect(screen.getByRole("button", { name: "Copy ada@example.com" })).toBeInTheDocument();
      expect(screen.queryByRole("group", { name: /Contact card/ })).not.toBeInTheDocument();
      expect(lookup).not.toHaveBeenCalled();
    });
  });

  describe("placeHoverCard", () => {
    const viewport = { width: 1000, height: 800 };
    const card = { width: 300, height: 120 };

    it("opens below the name, aligned to its left edge", () => {
      expect(placeHoverCard({ top: 100, bottom: 120, left: 200 }, card, viewport)).toEqual({ top: 128, left: 200 });
    });

    it("moves left to stay inside the window instead of running under the next pane", () => {
      expect(placeHoverCard({ top: 100, bottom: 120, left: 900 }, card, viewport)).toEqual({ top: 128, left: 692 });
      expect(placeHoverCard({ top: 100, bottom: 120, left: 0 }, card, viewport).left).toBe(8);
    });

    it("opens above the name near the bottom of the window", () => {
      expect(placeHoverCard({ top: 740, bottom: 760, left: 200 }, card, viewport)).toEqual({ top: 612, left: 200 });
    });
  });
});
