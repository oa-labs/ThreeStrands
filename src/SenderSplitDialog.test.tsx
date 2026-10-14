import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SenderSplitDialog } from "./SenderSplitDialog";

const target = { accountId: "you@example.com", email: "News@Launch.example", name: "Launch News" };

describe("SenderSplitDialog", () => {
  afterEach(cleanup);

  it("opens on the name, in the modal scope, with the rest of the app inert", async () => {
    const outside = document.createElement("button");
    document.body.append(outside);
    render(<SenderSplitDialog target={target} onCreate={vi.fn()} onClose={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "New Split" });
    const name = within(dialog).getByRole("textbox", { name: "Name" });
    await waitFor(() => expect(name).toHaveFocus());
    expect(name).toHaveValue("Launch News");
    expect(outside.inert).toBe(true);
    expect(dialog.closest("[data-shortcut-scope]") ?? dialog.querySelector("[data-shortcut-scope]")).toHaveAttribute("data-shortcut-scope", "modal");
    outside.remove();
  });

  it("creates nothing until submitted, then splits by address or by domain in the conversation's account", async () => {
    const onCreate = vi.fn().mockResolvedValue(undefined);
    render(<SenderSplitDialog target={target} onCreate={onCreate} onClose={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "New Split" });
    const match = within(dialog).getByRole("combobox", { name: "Match" });
    expect(onCreate).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Create Split" }));
    await waitFor(() => expect(onCreate).toHaveBeenCalledWith("Launch News", "pattern", "news@launch.example", "you@example.com"));

    fireEvent.change(match, { target: { value: "domain" } });
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Name" }), { target: { value: " Launch " } });
    // Cmd/Ctrl+Enter submits like the dialog's primary button.
    fireEvent.keyDown(within(dialog).getByRole("textbox", { name: "Name" }), { key: "Enter", metaKey: true });
    await waitFor(() => expect(onCreate).toHaveBeenLastCalledWith("Launch", "domain", "launch.example", "you@example.com"));
  });

  it("offers only the address for a personal mail provider and needs a name", () => {
    render(<SenderSplitDialog target={{ ...target, email: "pal@gmail.com", name: "pal@gmail.com" }} onCreate={vi.fn()} onClose={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "New Split" });
    const match = within(dialog).getByRole("combobox", { name: "Match" });
    expect(match).toBeDisabled();
    expect(within(match).getAllByRole("option").map((option) => option.textContent)).toEqual(["Address pal@gmail.com"]);
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Name" }), { target: { value: "  " } });
    expect(within(dialog).getByRole("button", { name: "Create Split" })).toBeDisabled();
  });

  it("keeps what was entered when saving fails, and closes with Cancel or Escape", async () => {
    const onClose = vi.fn();
    render(<SenderSplitDialog target={target} onCreate={vi.fn().mockRejectedValue(new Error("Split inbox name cannot be empty"))} onClose={onClose} />);
    const dialog = screen.getByRole("dialog", { name: "New Split" });
    const name = within(dialog).getByRole("textbox", { name: "Name" });
    fireEvent.change(name, { target: { value: "Launch" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create Split" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Split inbox name cannot be empty");
    expect(name).toHaveValue("Launch");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(2);
  });
});
