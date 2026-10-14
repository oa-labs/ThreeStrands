import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { MuteSenderDialog } from "./MuteSenderDialog";

const target = { accountId: "you@example.com", email: "News@Launch.example", name: "Launch News" };

describe("MuteSenderDialog", () => {
  afterEach(cleanup);

  it("opens on the confirm button in the modal scope and explains what muting does", async () => {
    const outside = document.createElement("button");
    document.body.append(outside);
    render(<MuteSenderDialog target={target} inboxThreadIds={["a"]} onMute={vi.fn()} onClose={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "Mute Sender" });
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Mute Sender" })).toHaveFocus());
    expect(outside.inert).toBe(true);
    expect(dialog.closest("[data-shortcut-scope]") ?? dialog.querySelector("[data-shortcut-scope]")).toHaveAttribute("data-shortcut-scope", "modal");
    expect(dialog).toHaveTextContent("New mail from news@launch.example will be marked read and archived as it arrives");
    outside.remove();
  });

  it("archives what is already in the Inbox by default and mutes nothing until confirmed", async () => {
    const onMute = vi.fn().mockResolvedValue(undefined);
    render(<MuteSenderDialog target={target} inboxThreadIds={["a", "b"]} onMute={onMute} onClose={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "Mute Sender" });
    const archive = within(dialog).getByRole("checkbox", { name: "Also archive the 2 conversations already in your Inbox" });
    expect(archive).toBeChecked();
    expect(onMute).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Mute Sender" }));
    await waitFor(() => expect(onMute).toHaveBeenCalledWith("address", "news@launch.example", ["a", "b"]));

    fireEvent.click(archive);
    fireEvent.change(within(dialog).getByRole("combobox", { name: "Mute" }), { target: { value: "domain" } });
    expect(dialog).toHaveTextContent("New mail from anyone at launch.example");
    fireEvent.keyDown(archive, { key: "Enter", ctrlKey: true });
    await waitFor(() => expect(onMute).toHaveBeenLastCalledWith("domain", "launch.example", []));
  });

  it("offers only the address for a personal provider, and no archive choice with an empty Inbox", () => {
    render(<MuteSenderDialog target={{ ...target, email: "pal@gmail.com" }} inboxThreadIds={[]} onMute={vi.fn()} onClose={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "Mute Sender" });
    const match = within(dialog).getByRole("combobox", { name: "Mute" });
    expect(match).toBeDisabled();
    expect(within(match).getAllByRole("option").map((option) => option.textContent)).toEqual(["Address pal@gmail.com"]);
    expect(within(dialog).queryByRole("checkbox")).not.toBeInTheDocument();
  });

  it("keeps the choices when muting fails, and closes with Cancel or Escape", async () => {
    const onClose = vi.fn();
    render(<MuteSenderDialog target={target} inboxThreadIds={["a"]} onMute={vi.fn().mockRejectedValue(new Error("You can't mute your own address"))} onClose={onClose} />);
    const dialog = screen.getByRole("dialog", { name: "Mute Sender" });
    const archive = within(dialog).getByRole("checkbox", { name: "Also archive the conversation already in your Inbox" });
    fireEvent.click(archive);
    fireEvent.click(within(dialog).getByRole("button", { name: "Mute Sender" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("You can't mute your own address");
    expect(archive).not.toBeChecked();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(2);
  });
});
