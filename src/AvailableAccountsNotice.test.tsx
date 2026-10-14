import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AvailableAccountsNotice } from "./AvailableAccountsNotice";
import type { Account } from "./domain";

const account: Account = {
  email: "personal@example.com",
  displayName: null,
  nickname: "Personal",
  color: "#112233",
  status: "needs_reauth",
  provider: "imap",
  labelModel: "imapLabelFolders",
  sortOrder: 1,
  connectedAt: "2026-10-08T12:00:00Z",
  lastSyncedAt: null,
  onThisDevice: false,
};

describe("available accounts notice", () => {
  afterEach(cleanup);

  it("offers an account from another device without calling it broken", () => {
    const onReview = vi.fn();
    const onDismiss = vi.fn();
    render(<AvailableAccountsNotice accounts={[account]} suppressed={false} onReview={onReview} onDismiss={onDismiss} />);
    const notice = screen.getByRole("status");
    expect(notice).toHaveTextContent("Personal is set up on another device. You can set it up here too.");
    expect(notice).not.toHaveTextContent(/reconnect|error/i);
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    expect(onReview).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(onDismiss).toHaveBeenCalled();
  });

  it("summarizes several accounts", () => {
    render(<AvailableAccountsNotice accounts={[account, { ...account, email: "side@example.com", nickname: null }]} suppressed={false} onReview={vi.fn()} onDismiss={vi.fn()} />);
    expect(screen.getByRole("status")).toHaveTextContent("2 accounts from your other devices can be set up here.");
  });

  it("stays hidden with nothing new or while the account list is open", () => {
    const { rerender } = render(<AvailableAccountsNotice accounts={[]} suppressed={false} onReview={vi.fn()} onDismiss={vi.fn()} />);
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    rerender(<AvailableAccountsNotice accounts={[account]} suppressed onReview={vi.fn()} onDismiss={vi.fn()} />);
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });
});
