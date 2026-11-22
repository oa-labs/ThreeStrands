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
    const signIn = await screen.findByText("me@proton.me", { selector: "span" });
    expect(signIn.closest("input, textarea")).toBeNull();
    expect(screen.getByText("Sign-in address")).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: "Sign-in address" })).not.toBeInTheDocument();
    expect((screen.getByLabelText("Address 1") as HTMLInputElement).value).toBe("work@proton.me");
  });

  it("offers a default From choice when there is a second address and saves only changed addresses", async () => {
    const signInOnly = { ...options, identities: options.identities.slice(0, 1) };
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue(signInOnly);
    const save = vi.spyOn(mailClient, "setImapIdentities").mockResolvedValue(options.identities);
    renderPanel();
    const replyTo = await screen.findByLabelText("Reply-To (optional)");
    const saveButton = screen.getByRole("button", { name: "Save addresses" });
    expect(replyTo).toHaveAccessibleDescription();
    expect(saveButton).toBeDisabled();
    expect(screen.queryByRole("combobox", { name: "Default From address" })).not.toBeInTheDocument();

    fireEvent.change(replyTo, { target: { value: "help@example.com" } });
    expect(saveButton).toBeEnabled();
    fireEvent.change(replyTo, { target: { value: "" } });
    expect(saveButton).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: "Add address" }));
    expect(saveButton).toBeDisabled();
    expect(screen.queryByRole("combobox", { name: "Default From address" })).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Address 1"), { target: { value: "work@proton.me" } });
    const defaultFrom = screen.getByRole("combobox", { name: "Default From address" });
    expect(defaultFrom).toHaveDisplayValue("me@proton.me");
    expect(within(defaultFrom).getByRole("option", { name: "work@proton.me" })).toBeInTheDocument();

    await act(async () => { fireEvent.click(saveButton); });
    expect(save).toHaveBeenCalledOnce();
    const sending = screen.getByRole("region", { name: "Sending addresses" });
    expect(within(sending).getByRole("status")).toHaveTextContent("Addresses saved.");
    expect(saveButton).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Remove address 1" }));
    expect(defaultFrom).not.toBeInTheDocument();
    expect(saveButton).toBeEnabled();
  });

  it("adds and removes secondary address rows and saves the full list", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue(options);
    const save = vi.spyOn(mailClient, "setImapIdentities").mockResolvedValue(options.identities);
    renderPanel();
    await screen.findByLabelText("Reply-To (optional)");
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
    const replyTo = await screen.findByLabelText("Reply-To (optional)");
    const sending = replyTo.closest("section")!;
    // Add a duplicate of the existing secondary address.
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: /Add address/ })); });
    const added = screen.getByLabelText("Address 2") as HTMLInputElement;
    await act(async () => { fireEvent.change(added, { target: { value: "work@proton.me" } }); });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Save addresses" })); });
    expect(await within(sending).findByRole("alert")).toHaveTextContent("listed more than once");
    // The user's edited rows are still shown (the component did not reload on error).
    expect((screen.getByLabelText("Address 1") as HTMLInputElement).value).toBe("work@proton.me");
    expect((screen.getByLabelText("Address 2") as HTMLInputElement).value).toBe("work@proton.me");
  });

  it("saves Reply-To for each sending address and a single default", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue(options);
    const save = vi.spyOn(mailClient, "setImapIdentities").mockResolvedValue(options.identities);
    renderPanel();
    const replyTo = await screen.findByLabelText("Reply-To (optional)");
    fireEvent.change(replyTo, { target: { value: "help@example.com" } });
    fireEvent.change(screen.getByLabelText("Reply-To 1"), { target: { value: "team@example.com" } });
    fireEvent.change(screen.getByLabelText("Default From address"), { target: { value: "0" } });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Save addresses" })); });
    expect(save).toHaveBeenCalledWith("me@proton.me", [
      { address: "me@proton.me", displayName: null, replyTo: "help@example.com" },
      { address: "work@proton.me", displayName: "Work", replyTo: "team@example.com", isDefault: true },
    ]);
  });

  it("toggles the server-saves-sent override", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue(options);
    const set = vi.spyOn(mailClient, "setImapServerSavesSent").mockResolvedValue();
    renderPanel();
    const fieldset = await screen.findByRole("group", { name: "Save sent messages" });
    const manual = within(fieldset).getByRole("radio", { name: "Use the server’s copy" });
    await act(async () => { fireEvent.click(manual); });
    expect(set).toHaveBeenCalledWith("me@proton.me", true);
    expect(manual).toBeChecked();
    expect(within(fieldset).getByRole("status")).toHaveTextContent("Sent settings saved.");
  });

  it("preserves a draft through changes to Sent mode and a scheduled re-check", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue({ ...options, sentCopyDetected: false });
    vi.spyOn(mailClient, "setImapServerSavesSent").mockResolvedValue();
    vi.spyOn(mailClient, "resetImapSentCopyDetection").mockResolvedValue();
    const save = vi.spyOn(mailClient, "setImapIdentities").mockResolvedValue(options.identities);
    renderPanel();

    const replyTo = await screen.findByLabelText("Reply-To (optional)");
    fireEvent.change(replyTo, { target: { value: "help@example.com" } });
    fireEvent.click(screen.getByRole("button", { name: "Add address" }));
    const alias = screen.getByLabelText("Address 2");
    fireEvent.change(alias, { target: { value: "alias@example.com" } });
    const defaultFrom = screen.getByRole("combobox", { name: "Default From address" });
    fireEvent.change(defaultFrom, { target: { value: "1" } });

    await act(async () => { fireEvent.click(screen.getByRole("radio", { name: "Use the server’s copy" })); });
    expect(screen.getByRole("radio", { name: "Use the server’s copy" })).toBeChecked();
    await act(async () => { fireEvent.click(screen.getByRole("radio", { name: "Automatic (recommended)" })); });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Re-check on next send" })); });
    expect(within(screen.getByRole("group", { name: "Save sent messages" })).getByRole("status"))
      .toHaveTextContent("ThreeStrands will check on your next send.");

    expect(replyTo).toHaveValue("help@example.com");
    expect(alias).toHaveValue("alias@example.com");
    expect(defaultFrom).toHaveDisplayValue("alias@example.com");
    expect(save).not.toHaveBeenCalled();
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Save addresses" })); });
    expect(save).toHaveBeenCalledWith("me@proton.me", [
      { address: "me@proton.me", displayName: null, replyTo: "help@example.com" },
      { address: "work@proton.me", displayName: "Work" },
      { address: "alias@example.com", displayName: null, isDefault: true },
    ]);
  });

  it("shows errors from a Sent action in the Sent section", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue(options);
    vi.spyOn(mailClient, "setImapServerSavesSent").mockRejectedValue(new Error("Could not save Sent settings."));
    renderPanel();
    const sent = await screen.findByRole("group", { name: "Save sent messages" });
    await act(async () => { fireEvent.click(within(sent).getByRole("radio", { name: "Use the server’s copy" })); });
    expect(within(sent).getByRole("alert")).toHaveTextContent("Could not save Sent settings.");
    expect(within(sent).getByRole("radio", { name: "Automatic (recommended)" })).toBeChecked();
  });

  it("stacks each sent-copy choice with its own description and leads with what the setting does", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue(options);
    renderPanel();
    const fieldset = await screen.findByRole("group", { name: "Save sent messages" });
    expect(fieldset).toHaveAccessibleDescription();
    const automatic = within(fieldset).getByRole("radio", { name: "Automatic (recommended)" });
    const manual = within(fieldset).getByRole("radio", { name: "Use the server’s copy" });
    expect(automatic).toBeChecked();
    expect(automatic).toHaveAccessibleDescription();
    // The risk of the manual choice is stated on that choice, not as the intro.
    expect(manual).toHaveAccessibleDescription(/sent messages won't appear in your Sent folder/);
    expect(fieldset.textContent).not.toMatch(/If this is wrong/);
  });

  it("says detection hasn't happened yet without offering a re-check", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue(options);
    renderPanel();
    expect(await screen.findByText("ThreeStrands will check on your next send.")).toHaveAttribute("role", "status");
    expect(screen.queryByRole("button", { name: /Re-check|Check again/ })).not.toBeInTheDocument();
  });

  it("shows each detection result and resets detection on Re-check on next send", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue({ ...options, sentCopyDetected: true });
    const reset = vi.spyOn(mailClient, "resetImapSentCopyDetection").mockResolvedValue();
    renderPanel();
    expect(await screen.findByText("Your mail server saves a copy in Sent.")).toHaveAttribute("role", "status");
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Re-check on next send" })); });
    await waitFor(() => expect(reset).toHaveBeenCalledWith("me@proton.me"));
    expect(screen.queryByRole("button", { name: "Re-check on next send" })).not.toBeInTheDocument();
    cleanup();
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue({ ...options, sentCopyDetected: false });
    renderPanel();
    expect(await screen.findByText("ThreeStrands saves a copy in Sent.")).toHaveAttribute("role", "status");
  });

  it("hides detection status when the user has said the server saves sent mail", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue({ ...options, serverSavesSentOverride: true, sentCopyDetected: false });
    renderPanel();
    expect(await screen.findByRole("radio", { name: "Use the server’s copy" })).toBeChecked();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Re-check on next send" })).not.toBeInTheDocument();
  });

  it("surfaces a load failure instead of silently blanking", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockRejectedValue(new Error("not an IMAP account"));
    renderPanel();
    expect(await screen.findByRole("alert")).toHaveTextContent("not an IMAP account");
  });
});
