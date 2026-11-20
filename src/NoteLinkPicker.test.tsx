import { fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import { NoteLinkPicker, type NoteLinkChoice } from "./NoteLinkPicker";

const thread: NoteLinkChoice = {
  label: "Re: Quarterly plan",
  detail: "Ada <ada@example.com>",
  sentAt: "2026-11-20T14:00:00Z",
  reference: { version: 1, kind: "conversation", id: "thread-q", account: "acct-1" },
};

describe("NoteLinkPicker", () => {
  afterEach(() => vi.restoreAllMocks());

  it("lists matches as one choice each, with the title and its detail kept separate", async () => {
    vi.spyOn(mailClient, "noteLinkChoices").mockResolvedValue([thread]);
    const onSelect = vi.fn().mockResolvedValue(undefined);
    render(<NoteLinkPicker onClose={vi.fn()} onSelect={onSelect} />);
    expect(screen.getByRole("status")).toHaveTextContent("Searching…");
    const list = within(await screen.findByRole("list", { name: "Matches" }));
    const choice = list.getByRole("button", { name: /Re: Quarterly plan/ });
    expect(within(choice).getByText("Re: Quarterly plan").tagName).toBe("SPAN");
    expect(within(choice).getByText(/Ada <ada@example.com>/).tagName).toBe("SMALL");
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    fireEvent.click(choice);
    expect(onSelect).toHaveBeenCalledWith(thread);
  });

  it("says when nothing matches only after the search has answered", async () => {
    vi.spyOn(mailClient, "noteLinkChoices").mockResolvedValue([]);
    render(<NoteLinkPicker onClose={vi.fn()} onSelect={vi.fn()} />);
    expect(screen.getByRole("status")).toHaveTextContent("Searching…");
    expect(await screen.findByText("No matching items on this device.")).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Matches" })).not.toBeInTheDocument();
  });

  it("shows a failed search as an error rather than as no matches", async () => {
    vi.spyOn(mailClient, "noteLinkChoices").mockRejectedValue(new Error("Search failed"));
    render(<NoteLinkPicker onClose={vi.fn()} onSelect={vi.fn()} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Search failed");
    expect(screen.queryByText("No matching items on this device.")).not.toBeInTheDocument();
  });
});
