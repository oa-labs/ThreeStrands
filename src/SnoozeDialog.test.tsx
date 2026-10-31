import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SnoozeDialog } from "./SnoozeDialog";

describe("SnoozeDialog", () => {
  it("rejects past dates, explains background behavior, and saves a future date", async () => {
    const save = vi.fn().mockResolvedValue(undefined);
    const close = vi.fn();
    render(<SnoozeDialog count={2} onSave={save} onClose={close} />);
    const input = screen.getByLabelText("Pick date & time");
    expect(input).toHaveAccessibleDescription(/ThreeStrands is running and online/);
    fireEvent.change(input, { target: { value: "2000-01-01T09:00" } });
    fireEvent.click(screen.getByRole("button", { name: "Snooze" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Choose a future return time");
    expect(save).not.toHaveBeenCalled();
    fireEvent.change(input, { target: { value: "2099-01-01T09:00" } });
    fireEvent.click(screen.getByRole("button", { name: "Snooze" }));
    await waitFor(() => expect(save).toHaveBeenCalledWith(new Date(2099, 0, 1, 9).toISOString()));
    await waitFor(() => expect(close).toHaveBeenCalled());
  });
  it("preserves a failed save for retry and preloads an existing snooze", async () => {
    const close = vi.fn();
    const save = vi.fn().mockRejectedValue(new Error("Storage unavailable"));
    render(<SnoozeDialog count={1} initialUntil="2099-01-01T09:00:00Z" onSave={save} onClose={close} />);
    expect(screen.getByRole("dialog", { name: "Reschedule Snooze" })).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Tomorrow" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Storage unavailable");
    expect(close).not.toHaveBeenCalled();
  });
});
