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
  it("renders the sign-in address read-only and the secondary addresses editable", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue(options);
    renderPanel();
    const signIn = await screen.findByLabelText("Sign-in address") as HTMLInputElement;
    expect(signIn.value).toBe("me@proton.me");
    expect(signIn).toHaveAttribute("readonly");
    expect((screen.getByLabelText("Address 1") as HTMLInputElement).value).toBe("work@proton.me");
  });

  it("adds and removes secondary address rows and saves the full list", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue(options);
    const save = vi.spyOn(mailClient, "setImapIdentities").mockResolvedValue(options.identities);
    renderPanel();
    await screen.findByLabelText("Sign-in address");
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
    await screen.findByLabelText("Sign-in address");
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
    await screen.findByLabelText("Sign-in address");
    const fieldset = screen.getByRole("group", { name: "Sent mail copy" });
    await act(async () => { fireEvent.click(within(fieldset).getByLabelText(/My mail server saves sent mail itself/)); });
    await waitFor(() => expect(set).toHaveBeenCalledWith("me@proton.me", true));
  });

  it("shows the detection status and resets detection on Check again", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockResolvedValue({ ...options, sentCopyDetected: true });
    const reset = vi.spyOn(mailClient, "resetImapSentCopyDetection").mockResolvedValue();
    renderPanel();
    expect(await screen.findByText("Your server saves a copy itself.")).toBeInTheDocument();
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Check again" })); });
    await waitFor(() => expect(reset).toHaveBeenCalledWith("me@proton.me"));
  });

  it("surfaces a load failure instead of silently blanking", async () => {
    vi.spyOn(mailClient, "getImapSendingOptions").mockRejectedValue(new Error("not an IMAP account"));
    renderPanel();
    expect(await screen.findByRole("alert")).toHaveTextContent("not an IMAP account");
  });
});
