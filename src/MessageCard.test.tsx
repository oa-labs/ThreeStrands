import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ContactCardContext, type ContactCardActions } from "./ContactCard";
import type { OutboxItem } from "./correspondence";
import { mailClient } from "./data/client";
import type { Account, ContactProfile, Message } from "./domain";
import * as inlineAttachments from "./inlineAttachments";
import { placeHoverCard } from "./AddressWithCard";
import { MessageCard } from "./MessageCard";
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

  it("lists the first 20 recipients of a long list and reveals the rest on request", () => {
    const addresses = (count: number) => Array.from({ length: count }, (_, i) => `Person ${i + 1} <p${i + 1}@example.com>`);
    const { container, rerender } = render(<MessageCard message={{ ...message, recipients: addresses(20) }} isExpanded {...stableProps} />);
    const line = () => container.querySelector(".message-recipients")!;
    // Exactly at the limit, every recipient shows.
    expect(line()).toHaveTextContent(/Person 19p19@example\.com, and Person 20p20@example\.com$/);
    expect(screen.queryByRole("button", { name: /more$/ })).not.toBeInTheDocument();

    rerender(<MessageCard message={{ ...message, id: "m2", recipients: addresses(105) }} isExpanded {...stableProps} />);
    expect(line()).toHaveTextContent(/Person 20p20@example\.com, and 85 more$/);
    expect(line()).not.toHaveTextContent("p21@example.com");
    fireEvent.click(screen.getByRole("button", { name: "85 more" }));
    expect(line()).toHaveTextContent(/Person 104p104@example\.com, and Person 105p105@example\.com$/);
    expect(screen.queryByRole("button", { name: /more$/ })).not.toBeInTheDocument();
    expect(stableProps.onToggle).not.toHaveBeenCalled();
  });

  describe("recipient line", () => {
    const own = [{ email: "me@example.com" } as Account];
    const names = (container: HTMLElement) =>
      Array.from(container.querySelectorAll(".message-recipients .address-name"), (node) => node.textContent);
    const line = (container: HTMLElement) => container.querySelector(".message-recipients")!;

    it("shows Cc after To and calls the user's own address me", () => {
      const { container } = render(<MessageCard
        message={{ ...message, recipients: ["David Martinez <david@example.com>"], cc: ["Joel <ME@example.com>", "Jane Doe <jane@example.com>"] }}
        isExpanded {...stableProps} accounts={own}
      />);
      expect(names(container)).toEqual(["David Martinez", "me", "Jane Doe"]);
      expect(line(container)).toHaveTextContent(/^to David Martinezdavid@example\.com, cc meME@example\.com and Jane Doejane@example\.com$/);
    });

    it("says to me when the user is a To recipient and omits an empty Cc", () => {
      const { container } = render(<MessageCard message={{ ...message, cc: [] }} isExpanded {...stableProps} accounts={own} />);
      expect(names(container)).toEqual(["me"]);
      expect(line(container)).not.toHaveTextContent("cc");
    });

    it("shows only the Cc group when a message has no To", () => {
      const { container } = render(<MessageCard
        message={{ ...message, recipients: [], cc: ["me@example.com"] }}
        isExpanded {...stableProps} accounts={own}
      />);
      expect(line(container)).toHaveTextContent(/^cc meme@example\.com$/);
    });

    it("counts To and Cc together toward the preview limit", () => {
      const people = (prefix: string, count: number) => Array.from({ length: count }, (_, i) => `${prefix} ${i + 1} <${prefix}${i + 1}@example.com>`);
      const { container } = render(<MessageCard
        message={{ ...message, recipients: people("To", 15), cc: people("Cc", 10) }}
        isExpanded {...stableProps}
      />);
      expect(names(container)).toHaveLength(20);
      expect(line(container)).toHaveTextContent(/, cc Cc 1Cc1@example\.com, .*Cc 5Cc5@example\.com, and 5 more$/);
      fireEvent.click(screen.getByRole("button", { name: "5 more" }));
      expect(names(container)).toHaveLength(25);
      expect(line(container)).toHaveTextContent(/Cc 9Cc9@example\.com, and Cc 10Cc10@example\.com$/);
    });
  });

  it("reports toggles and responses by message id", () => {
    render(<MessageCard message={message} isExpanded {...stableProps} />);
    fireEvent.click(screen.getByRole("button", { name: "Reply All" }));
    fireEvent.click(screen.getByRole("button", { name: /Collapse message from/ }));

    expect(stableProps.onRespond).toHaveBeenCalledWith("replyAll", "m1");
    expect(stableProps.onToggle).toHaveBeenCalledWith("m1", true);
  });

  it("offers Edit as New only on a message one of the accounts sent", () => {
    const onEditAsNew = vi.fn();
    const accounts = [{ email: "me@example.com" }] as Account[];
    const { unmount } = render(<MessageCard message={message} isExpanded {...stableProps} accounts={accounts} onEditAsNew={onEditAsNew} />);
    expect(screen.queryByRole("button", { name: "Edit as New" })).toBeNull();
    unmount();

    render(<MessageCard message={{ ...message, sender: "Me <ME@example.com>" }} isExpanded {...stableProps} accounts={accounts} onEditAsNew={onEditAsNew} />);
    fireEvent.click(screen.getByRole("button", { name: "Edit as New" }));
    expect(onEditAsNew).toHaveBeenCalledWith("m1");
    expect(stableProps.onRespond).not.toHaveBeenCalled();
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

  describe("attachment chips", () => {
    const photo = { id: "a-photo", filename: "Joel.png", mimeType: "image/png", size: 135_000 };
    const vector = { id: "a-vector", filename: "logo.svg", mimeType: "image/svg+xml", size: 4_000 };
    const report = { id: "a-report", filename: "report.pdf", mimeType: "application/pdf", size: 90_000 };
    const withAttachments = (attachments: Message["attachments"]): Message => ({ ...message, attachments });

    it("names one verb per target, in a tooltip and an accessible name, with the star last", () => {
      vi.spyOn(mailClient, "fileStars").mockResolvedValue([]);
      render(<MessageCard message={withAttachments([report])} isExpanded {...stableProps} />);
      const chip = screen.getByRole("button", { name: "Open report.pdf" }).closest(".message-attachment") as HTMLElement;
      const buttons = within(chip).getAllByRole("button");
      expect(buttons.map((button) => button.getAttribute("aria-label"))).toEqual(["Open report.pdf", "Save report.pdf as…", "Star report.pdf"]);
      expect(within(chip).getAllByRole("tooltip", { hidden: true }).map((tip) => tip.textContent)).toEqual([
        "Open report.pdf",
        "Save report.pdf as…",
        "Star for later · stored on this device",
      ]);
      // A native title would show a second, unstyled tooltip beside the custom one.
      expect(chip.querySelector("[title]")).toBeNull();
    });

    it("opens from the filename and saves from the save segment", async () => {
      vi.spyOn(mailClient, "fileStars").mockResolvedValue([]);
      const open = vi.spyOn(mailClient, "openAttachment").mockResolvedValue();
      const save = vi.spyOn(mailClient, "saveAttachment").mockResolvedValue();
      render(<MessageCard message={withAttachments([report])} isExpanded {...stableProps} />);
      fireEvent.click(screen.getByRole("button", { name: "Open report.pdf" }));
      fireEvent.click(screen.getByRole("button", { name: "Save report.pdf as…" }));
      expect(open).toHaveBeenCalledWith("m1", "a-report");
      expect(save).toHaveBeenCalledWith("m1", "a-report");
    });

    it("marks a risky file type on its chip and leaves confirmation to the native open", async () => {
      vi.spyOn(mailClient, "fileStars").mockResolvedValue([]);
      const open = vi.spyOn(mailClient, "openAttachment").mockResolvedValue();
      const risky = { id: "a-risky", filename: "invoice.pdf.exe", mimeType: "application/octet-stream", size: 9_000 };
      render(<MessageCard message={withAttachments([report, risky])} isExpanded {...stableProps} />);
      const badge = screen.getByRole("button", { name: "Open invoice.pdf.exe" });
      expect(badge).toHaveAccessibleDescription(/hides its real type/);
      expect(badge.querySelector(".attachment-risk")).not.toBeNull();
      expect(screen.getByRole("button", { name: "Open report.pdf" }).querySelector(".attachment-risk")).toBeNull();
      // The native open command reads the file and asks before opening it; a
      // second prompt here would make people confirm twice.
      fireEvent.click(badge);
      expect(open).toHaveBeenCalledWith("m1", "a-risky");
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });

    it("shows a supported raster's picture and keeps the kind icon for other files", async () => {
      vi.spyOn(mailClient, "fileStars").mockResolvedValue([]);
      const fetch = vi.spyOn(mailClient, "fetchAttachmentImage").mockResolvedValue("data:image/png;base64,AAAA");
      render(<MessageCard message={withAttachments([photo, vector, report])} isExpanded {...stableProps} />);
      const photoButton = screen.getByRole("button", { name: "Open Joel.png" });
      await waitFor(() => expect(photoButton.querySelector("img.attachment-thumbnail")).toHaveAttribute("src", "data:image/png;base64,AAAA"));
      expect(fetch).toHaveBeenCalledTimes(1);
      expect(fetch).toHaveBeenCalledWith("m1", "a-photo");
      // SVG can carry script and remote references, so it never becomes a thumbnail.
      expect(screen.getByRole("button", { name: "Open logo.svg" }).querySelector("img")).toBeNull();
      expect(screen.getByRole("button", { name: "Open logo.svg" }).querySelector("[data-attachment-kind]")).toHaveAttribute("data-attachment-kind", "image");
      expect(screen.getByRole("button", { name: "Open report.pdf" }).querySelector("img")).toBeNull();
    });

    it("keeps the kind icon when the picture is refused or is not an image data URI", async () => {
      vi.spyOn(mailClient, "fileStars").mockResolvedValue([]);
      const second = { ...photo, id: "a-photo-2", filename: "Greg.jpg", mimeType: "image/jpeg" };
      vi.spyOn(mailClient, "fetchAttachmentImage").mockImplementation(async (_message, attachment) => {
        if (attachment === photo.id) throw new Error("Image exceeds the maximum allowed size");
        return "https://tracker.example/pixel.png";
      });
      render(<MessageCard message={withAttachments([photo, second])} isExpanded {...stableProps} />);
      await waitFor(() => expect(mailClient.fetchAttachmentImage).toHaveBeenCalledTimes(2));
      await act(async () => {});
      for (const name of ["Open Joel.png", "Open Greg.jpg"]) {
        const button = screen.getByRole("button", { name });
        expect(button.querySelector("img")).toBeNull();
        expect(button.querySelector("[data-attachment-kind]")).toHaveAttribute("data-attachment-kind", "image");
      }
    });

    it("requests no thumbnails for a reply still in the outbox", () => {
      vi.spyOn(mailClient, "fileStars").mockResolvedValue([]);
      const fetch = vi.spyOn(mailClient, "fetchAttachmentImage");
      const queuedItem = { draft: { id: "d1" } } as unknown as OutboxItem;
      render(<MessageCard message={withAttachments([photo])} isExpanded {...stableProps} queuedItem={queuedItem} />);
      expect(fetch).not.toHaveBeenCalled();
    });
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
