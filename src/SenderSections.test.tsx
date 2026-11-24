import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { SenderFeedback } from "./SenderSections";
import { mailClient } from "./data/client";
import type { SenderDeliveryPattern } from "./domain";
import type { TrustReport } from "./senderSignals";

vi.mock("./data/client", () => ({ mailClient: { confirmSenderDelivery: vi.fn(), forgetSenderDelivery: vi.fn() } }));

const pattern = (id: string, linkOrigins: string[]): SenderDeliveryPattern => ({
  id, senderEmail: "requests@signing.example", replyTo: null, linkOrigins, authenticated: true,
  sourceMessageId: `m-${id}`, confirmedAt: "2026-10-26T12:00:00Z",
});

type Feedback = NonNullable<TrustReport["feedback"]>;
const feedback = (overrides: Partial<Feedback>): Feedback => ({
  requester: "requests@signing.example", replyTo: null, linkOrigins: ["https://a.example"],
  patterns: [], matching: null, covered: false, relevant: true, ...overrides,
});

describe("SenderFeedback", () => {
  afterEach(() => { cleanup(); vi.clearAllMocks(); });

  it("offers trust, not forgetting, when the message is not covered", () => {
    render(<SenderFeedback messageId="m" feedback={feedback({ patterns: [pattern("p1", ["https://b.example"])] })} />);
    expect(screen.getByRole("button", { name: "Trust this address for this account" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Forget confirmation" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Trust this address for this account" })).toHaveAccessibleDescription(expect.stringContaining("a year"));
  });

  it("offers no trust button when earlier confirmations cover the message, and lets each be forgotten", async () => {
    const first = pattern("p1", ["https://a.example"]);
    const second = pattern("p2", ["https://b.example"]);
    vi.mocked(mailClient.forgetSenderDelivery).mockResolvedValue(undefined);
    render(<SenderFeedback messageId="m" feedback={feedback({ covered: true, patterns: [first, second] })} />);
    expect(screen.queryByRole("button", { name: "Trust this address for this account" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Forget confirmation" })).not.toBeInTheDocument();
    expect(screen.getByText("Remembered senders")).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole("button", { name: /Forget confirmation for/ })[1]);
    await waitFor(() => expect(mailClient.forgetSenderDelivery).toHaveBeenCalledWith("p2"));
  });
});
