import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { GmailSendingSettings } from "./GmailSendingSettings";
import { mailClient } from "./data/client";

describe("GmailSendingSettings", () => {
  it("shows cached verified addresses and refreshes the default and reply destination", async () => {
    vi.spyOn(mailClient, "listIdentities").mockResolvedValue([{ address: "me@example.com" }]);
    const refresh = vi.spyOn(mailClient, "refreshGmailAliases").mockResolvedValue([{ address: "me@example.com" }, { address: "alias@example.org", isDefault: true, replyTo: "replies@example.org" }]);
    render(<GmailSendingSettings account="me@example.com" />);
    expect(await screen.findByText("me@example.com")).toBeVisible();
    const button = screen.getByRole("button", { name: "Refresh from Gmail" });
    expect(button).toHaveAccessibleDescription(/Only verified aliases/);
    expect(screen.getByRole("region", { name: "Sending Addresses for me@example.com" })).toBeVisible();
    fireEvent.click(button);
    await waitFor(() => expect(refresh).toHaveBeenCalledWith("me@example.com"));
    expect(await screen.findByText("alias@example.org")).toBeVisible();
    expect(screen.getByText("Default for new messages · Replies to replies@example.org")).toBeVisible();
  });
  it("keeps cached addresses when Gmail refresh fails", async () => {
    vi.spyOn(mailClient, "listIdentities").mockResolvedValue([{ address: "cached@example.org" }]);
    vi.spyOn(mailClient, "refreshGmailAliases").mockRejectedValue(new Error("Connect to Gmail"));
    render(<GmailSendingSettings account="me@example.com" />);
    await screen.findByText("cached@example.org");
    fireEvent.click(screen.getByRole("button", { name: "Refresh from Gmail" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Connect to Gmail");
    expect(screen.getByText("cached@example.org")).toBeVisible();
  });
});
