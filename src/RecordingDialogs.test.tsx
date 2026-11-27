import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { QuitRecordingDialog, RecordingConsentDialog } from "./RecordingDialogs";

describe("RecordingConsentDialog", () => {
  afterEach(cleanup);

  it("opens on Start Recording in the modal scope, with the rest of the app inert", async () => {
    const outside = document.createElement("button");
    document.body.append(outside);
    render(<RecordingConsentDialog onAccept={vi.fn().mockResolvedValue(undefined)} onCancel={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "Recording Meetings" });
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Start Recording" })).toHaveFocus());
    expect(outside.inert).toBe(true);
    expect(dialog).toHaveAttribute("data-shortcut-scope", "modal");
    // The promises the dialog makes before anything is captured.
    expect(dialog).toHaveTextContent("Recording laws differ by place");
    expect(dialog).toHaveTextContent("Tell the people in the meeting");
    expect(dialog).toHaveTextContent("The audio stays on this Mac");
    outside.remove();
  });

  it("keeps Tab inside the dialog", () => {
    render(<RecordingConsentDialog onAccept={vi.fn().mockResolvedValue(undefined)} onCancel={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "Recording Meetings" });
    const start = within(dialog).getByRole("button", { name: "Start Recording" });
    const close = within(dialog).getByRole("button", { name: "Close" });
    start.focus();
    fireEvent.keyDown(start, { key: "Tab" });
    expect(close).toHaveFocus();
    fireEvent.keyDown(close, { key: "Tab", shiftKey: true });
    expect(start).toHaveFocus();
  });

  it("accepts with Start Recording or Cmd/Ctrl+Enter, and not before", async () => {
    const onAccept = vi.fn().mockResolvedValue(undefined);
    render(<RecordingConsentDialog onAccept={onAccept} onCancel={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "Recording Meetings" });
    expect(onAccept).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Start Recording" }));
    await waitFor(() => expect(onAccept).toHaveBeenCalledTimes(1));
    fireEvent.keyDown(within(dialog).getByRole("button", { name: "Cancel" }), { key: "Enter", metaKey: true });
    await waitFor(() => expect(onAccept).toHaveBeenCalledTimes(2));
    fireEvent.keyDown(within(dialog).getByRole("button", { name: "Cancel" }), { key: "Enter", ctrlKey: true });
    await waitFor(() => expect(onAccept).toHaveBeenCalledTimes(3));
  });

  it("cancels with Cancel or Escape and returns focus to where it was", async () => {
    const opener = document.createElement("button");
    document.body.append(opener);
    opener.focus();
    const onCancel = vi.fn();
    const { unmount } = render(<RecordingConsentDialog onAccept={vi.fn()} onCancel={onCancel} />);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onCancel).toHaveBeenCalledTimes(2);
    unmount();
    expect(opener).toHaveFocus();
    opener.remove();
  });
});

describe("QuitRecordingDialog", () => {
  afterEach(cleanup);

  it("starts on Cancel so Enter never quits by accident", async () => {
    render(<QuitRecordingDialog onConfirm={vi.fn().mockResolvedValue(undefined)} onCancel={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "Stop Recording and Quit?" });
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Cancel" })).toHaveFocus());
  });

  it("confirms only with Stop and Quit", async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);
    const onCancel = vi.fn();
    render(<QuitRecordingDialog onConfirm={onConfirm} onCancel={onCancel} />);
    const dialog = screen.getByRole("dialog", { name: "Stop Recording and Quit?" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onCancel).toHaveBeenCalledTimes(2);
    expect(onConfirm).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Stop and Quit" }));
    await waitFor(() => expect(onConfirm).toHaveBeenCalledOnce());
  });
});
