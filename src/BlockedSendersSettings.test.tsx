import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { SenderRule } from "./domain";
import { BlockedSendersSettings } from "./BlockedSendersSettings";
import { MutedSendersSettings } from "./MutedSendersSettings";

const block = (id: string, accountId: string, address: string, lastMatchedAt?: string): SenderRule =>
  ({ id, accountId, matchKind: "address", matchValue: address, action: "block", createdAt: "2026-10-01T00:00:00Z", ...(lastMatchedAt ? { lastMatchedAt } : {}) });
const mute: SenderRule = { id: "m", accountId: "you@example.com", matchKind: "domain", matchValue: "launch.example", action: "mute", createdAt: "2026-10-01T00:00:00Z" };

describe("BlockedSendersSettings", () => {
  afterEach(cleanup);

  it("lists blocked senders by account with when they last wrote, and unblocks one", async () => {
    const onUnblock = vi.fn().mockResolvedValue(undefined);
    render(<BlockedSendersSettings
      rules={[mute, block("b", "work@example.org", "prize@lottery.example"), block("a", "you@example.com", "deals@spam.example", "2026-10-20T09:00:00Z")]}
      onUnblock={onUnblock}
    />);
    const section = screen.getByRole("region", { name: "Blocked Senders" });
    const items = within(section).getAllByRole("listitem");
    expect(items.map((item) => item.querySelector("strong")?.textContent)).toEqual(["prize@lottery.example", "deals@spam.example"]);
    expect(items[0]).toHaveTextContent("No email since blocking");
    expect(items[1]).toHaveTextContent("Last email");
    expect(items[1]).not.toHaveTextContent("No email since blocking");
    fireEvent.click(within(section).getByRole("button", { name: "Unblock prize@lottery.example" }));
    await waitFor(() => expect(onUnblock).toHaveBeenCalledWith("b"));
  });

  it("says how senders get blocked when none are, and shows a failed unblock", async () => {
    const { rerender } = render(<BlockedSendersSettings rules={[mute]} onUnblock={vi.fn()} />);
    expect(within(screen.getByRole("region", { name: "Blocked Senders" })).queryAllByRole("listitem")).toEqual([]);
    rerender(<BlockedSendersSettings rules={[block("a", "you@example.com", "prize@lottery.example")]} onUnblock={vi.fn().mockRejectedValue(new Error("Offline"))} />);
    fireEvent.click(screen.getByRole("button", { name: "Unblock prize@lottery.example" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Offline");
  });

  it("keeps blocks out of the mute list", () => {
    render(<MutedSendersSettings rules={[mute, block("b", "you@example.com", "prize@lottery.example")]} onUnmute={vi.fn()} />);
    const items = within(screen.getByRole("region", { name: "Muted Senders" })).getAllByRole("listitem");
    expect(items.map((item) => item.querySelector("strong")?.textContent)).toEqual(["Anyone at launch.example"]);
  });
});
