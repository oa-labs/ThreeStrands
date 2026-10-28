import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ImapSendingSettings } from "./ImapSendingSettings";
import { mailClient } from "./data/client";
import type { ImapSendingOptions } from "./domain";

const options: ImapSendingOptions = {
  identities: [
    { address: "me@proton.me" },
    { address: "work@proton.me", displayName: "Work" },
  ],
  serverSavesSentOverride: false,
  sentCopyDetected: null,
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderPanel() {
  return render(<ImapSendingSettings account="me@proton.me" />);
}

describe("ImapSendingSettings", () => {
  it("shows the sign-in address as text, not a field, and the secondary addresses editable", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue(options);
    renderPanel();
    const signIn = await screen.findByText("me@proton.me");
    expect(signIn.closest("input, textarea")).toBeNull();
    expect(screen.getByText(/Sign-in address · always available/)).toBeInTheDocument();
    expect(screen.queryByDisplayValue("me@proton.me")).not.toBeInTheDocument();
    expect((screen.getByLabelText("Address 1") as HTMLInputElement).value).toBe("work@proton.me");
  });

  it("adds and removes secondary address rows and saves the full list", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue(options);
    const save = vi.spyOn(mailClient, "setImapIdentities").mockResolvedValue(options.identities);
    renderPanel();
    await screen.findByText("me@proton.me");
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: /Add address/ })); });
    const second = screen.getByLabelText("Address 2") as HTMLInputElement;
    await act(async () => { fireEvent.change(second, { target: { value: "alias@proton.me" } }); });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Save addresses" })); });
    await waitFor(() => expect(save).toHaveBeenCalledWith("me@proton.me", [
      { address: "me@proton.me", displayName: null },
      { address: "work@proton.me", displayName: "Work" },
      { address: "alias@proton.me", displayName: null },
    ]));
    // Removing a row drops it from the next save.
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Remove address 1" })); });
    expect(screen.queryByLabelText("Address 1")).not.toBeInTheDocument();
  });

  it("shows a backend validation error and leaves the edited list in place", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue(options);
    vi.spyOn(mailClient, "setImapIdentities").mockRejectedValue(new Error("work@proton.me is listed more than once. Each sending address can appear only once."));
    renderPanel();
    await screen.findByText("me@proton.me");
    // Add a duplicate of the existing secondary address.
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: /Add address/ })); });
    const added = screen.getByLabelText("Address 2") as HTMLInputElement;
    await act(async () => { fireEvent.change(added, { target: { value: "work@proton.me" } }); });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Save addresses" })); });
    expect(await screen.findByRole("alert")).toHaveTextContent("listed more than once");
    // The user's edited rows are still shown (the component did not reload on error).
    expect((screen.getByLabelText("Address 1") as HTMLInputElement).value).toBe("work@proton.me");
    expect((screen.getByLabelText("Address 2") as HTMLInputElement).value).toBe("work@proton.me");
  });

  it("toggles the server-saves-sent override", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue(options);
    const set = vi.spyOn(mailClient, "setImapServerSavesSent").mockResolvedValue();
    renderPanel();
    await screen.findByText("me@proton.me");
    const fieldset = screen.getByRole("group", { name: "Sent mail copy" });
    await act(async () => { fireEvent.click(within(fieldset).getByLabelText(/My mail server saves sent mail itself/)); });
    await waitFor(() => expect(set).toHaveBeenCalledWith("me@proton.me", true));
  });

  it("stacks each sent-copy choice with its own description and leads with what the setting does", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue(options);
    renderPanel();
    await screen.findByText("me@proton.me");
    const fieldset = screen.getByRole("group", { name: "Sent mail copy" });
    expect(fieldset).toHaveAccessibleDescription();
    const automatic = within(fieldset).getByRole("radio", { name: "Automatic (recommended)" });
    const manual = within(fieldset).getByRole("radio", { name: "My mail server saves sent mail itself" });
    expect(automatic).toBeChecked();
    expect(automatic).toHaveAccessibleDescription();
    // The risk of the manual choice is stated on that choice, not as the intro.
    expect(manual).toHaveAccessibleDescription(/sent messages won't appear in your Sent folder/);
    expect(fieldset.textContent).not.toMatch(/If this is wrong/);
  });

  it("says detection hasn't happened yet without offering a re-check", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue(options);
    renderPanel();
    expect(await screen.findByText("Not checked yet. ThreeStrands checks when you send your next message.")).toHaveAttribute("role", "status");
    expect(screen.queryByRole("button", { name: /Re-check|Check again/ })).not.toBeInTheDocument();
  });

  it("shows each detection result and resets detection on Re-check on next send", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue({ ...options, sentCopyDetected: true });
    const reset = vi.spyOn(mailClient, "resetImapSentCopyDetection").mockResolvedValue();
    renderPanel();
    expect(await screen.findByText("Checked: your server keeps sent mail, so ThreeStrands doesn't save a copy.")).toHaveAttribute("role", "status");
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Re-check on next send" })); });
    await waitFor(() => expect(reset).toHaveBeenCalledWith("me@proton.me"));
    cleanup();
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue({ ...options, sentCopyDetected: false });
    renderPanel();
    expect(await screen.findByText("Checked: your server doesn't keep sent mail, so ThreeStrands saves a copy for you.")).toHaveAttribute("role", "status");
  });

  it("hides detection status when the user has said the server saves sent mail", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue({ ...options, serverSavesSentOverride: true, sentCopyDetected: false });
    renderPanel();
    await screen.findByText("me@proton.me");
    expect(screen.getByRole("radio", { name: "My mail server saves sent mail itself" })).toBeChecked();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Re-check on next send" })).not.toBeInTheDocument();
  });

  it("surfaces a load failure instead of silently blanking", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockRejectedValue(new Error("not an IMAP account"));
    renderPanel();
    expect(await screen.findByRole("alert")).toHaveTextContent("not an IMAP account");
  });
});
