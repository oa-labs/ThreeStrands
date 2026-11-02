import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { GmailSendingSettings } from "./GmailSendingSettings";
import { mailClient } from "./data/client";

describe("GmailSendingSettings", () => {
  it("shows cached verified addresses and refreshes the default and reply destination", async () => {
    vi.spyOn(mailClient, "listIdentities").mockResolvedValue([{ address: "me@example.com" }]);
    const refresh = vi.spyOn(mailClient, "refreshGmailAliases").mockResolvedValue([{ address: "me@example.com" }, { address: "alias@example.org", isDefault: true, replyTo: "replies@example.org" }]);
    render(<GmailSendingSettings account="me@example.com" />);
    await waitFor(() => expect(mailClient.listIdentities).toHaveBeenCalledWith("me@example.com"));
    fireEvent.click(screen.getByRole("button", { name: "Aliases…" }));
    const button = screen.getByRole("button", { name: "Refresh from Gmail" });
    expect(button).toHaveAccessibleDescription(/Only verified aliases/);
    expect(screen.getByRole("region", { name: "Sending Addresses for me@example.com" })).toBeVisible();
    fireEvent.click(button);
    await waitFor(() => expect(refresh).toHaveBeenCalledWith("me@example.com"));
    expect(await screen.findByText("alias@example.org")).toBeVisible();
    expect(screen.getByText("Default for new messages · Replies to replies@example.org")).toBeVisible();
  });
  it("keeps a primary-only account compact without repeating its address or default", async () => {
    vi.spyOn(mailClient, "listIdentities").mockResolvedValue([{ address: "ME@example.com", isDefault: true }]);
    render(<GmailSendingSettings account="me@example.com" />);
    await waitFor(() => expect(mailClient.listIdentities).toHaveBeenCalledWith("me@example.com"));
    const aliases = screen.getByRole("button", { name: "Aliases…" });
    expect(aliases).toHaveAttribute("aria-expanded", "false");
    expect(document.getElementById(aliases.getAttribute("aria-controls")!)).not.toBeVisible();
    expect(screen.queryByRole("heading", { name: "Sending Addresses" })).not.toBeInTheDocument();
    expect(screen.queryByText("ME@example.com")).not.toBeInTheDocument();
    expect(screen.queryByText("Default for new messages")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Refresh from Gmail" })).not.toBeInTheDocument();
    fireEvent.click(aliases);
    expect(aliases).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("button", { name: "Refresh from Gmail" })).toHaveAccessibleDescription(/Only verified aliases/);
    expect(screen.queryByRole("list")).not.toBeInTheDocument();
    fireEvent.click(aliases);
    expect(aliases).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("button", { name: "Refresh from Gmail" })).not.toBeInTheDocument();
  });
  it("automatically shows cached aliases without opening the disclosure", async () => {
    vi.spyOn(mailClient, "listIdentities").mockResolvedValue([{ address: "me@example.com", isDefault: true }, { address: "work@example.org" }]);
    render(<GmailSendingSettings account="me@example.com" />);
    expect(await screen.findByText("work@example.org")).toBeVisible();
    expect(screen.getByRole("heading", { name: "Sending Addresses" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Refresh from Gmail" })).toBeVisible();
    expect(screen.getByText("Default for new messages")).toBeVisible();
    expect(screen.queryByRole("button", { name: "Aliases…" })).not.toBeInTheDocument();
  });
  it("keeps a custom Reply-To visible on a primary-only account", async () => {
    vi.spyOn(mailClient, "listIdentities").mockResolvedValue([{ address: "me@example.com", replyTo: "replies@example.org" }]);
    render(<GmailSendingSettings account="me@example.com" />);
    expect(await screen.findByText("Replies to replies@example.org")).toBeVisible();
    expect(screen.getByRole("button", { name: "Aliases…" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("heading", { name: "Sending Addresses" })).not.toBeInTheDocument();
  });
  it("returns to the compact control when refresh removes the last alias", async () => {
    vi.spyOn(mailClient, "listIdentities").mockResolvedValue([{ address: "me@example.com" }, { address: "old@example.org" }]);
    vi.spyOn(mailClient, "refreshGmailAliases").mockResolvedValue([{ address: "me@example.com", isDefault: true }]);
    render(<GmailSendingSettings account="me@example.com" />);
    await screen.findByText("old@example.org");
    fireEvent.click(screen.getByRole("button", { name: "Refresh from Gmail" }));
    expect(await screen.findByRole("button", { name: "Aliases…" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("old@example.org")).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Sending Addresses" })).not.toBeInTheDocument();
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
