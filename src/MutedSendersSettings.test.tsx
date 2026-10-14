import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { SenderRule } from "./domain";
import { MutedSendersSettings } from "./MutedSendersSettings";

const rule = (id: string, accountId: string, matchKind: SenderRule["matchKind"], matchValue: string): SenderRule =>
  ({ id, accountId, matchKind, matchValue, action: "mute", createdAt: "2026-10-01T00:00:00Z" });

describe("MutedSendersSettings", () => {
  afterEach(cleanup);

  it("lists muted senders by account and unmutes one", async () => {
    const onUnmute = vi.fn().mockResolvedValue(undefined);
    render(<MutedSendersSettings
      rules={[rule("b", "work@example.org", "address", "alerts@ops.example"), rule("a", "you@example.com", "domain", "launch.example")]}
      onUnmute={onUnmute}
    />);
    const section = screen.getByRole("region", { name: "Muted Senders" });
    const items = within(section).getAllByRole("listitem");
    expect(items.map((item) => item.querySelector("strong")?.textContent)).toEqual(["alerts@ops.example", "Anyone at launch.example"]);
    expect(items[1]).toHaveTextContent("you@example.com");
    fireEvent.click(within(section).getByRole("button", { name: "Unmute alerts@ops.example" }));
    await waitFor(() => expect(onUnmute).toHaveBeenCalledWith("b"));
  });

  it("says where to mute when nothing is muted, and shows a failed unmute", async () => {
    const { rerender } = render(<MutedSendersSettings rules={[]} onUnmute={vi.fn()} />);
    expect(screen.getByRole("region", { name: "Muted Senders" })).toHaveTextContent("choose Mute in the side panel");
    rerender(<MutedSendersSettings rules={[rule("a", "you@example.com", "address", "news@launch.example")]} onUnmute={vi.fn().mockRejectedValue(new Error("Offline"))} />);
    fireEvent.click(screen.getByRole("button", { name: "Unmute news@launch.example" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Offline");
  });
});
