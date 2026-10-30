import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AiDiagnosticsSettings } from "./AiDiagnosticsSettings";
import { mailClient } from "./data/client";
import type { AiDiagnosticSnapshot } from "./domain";

const entry: AiDiagnosticSnapshot["entries"][number] = {
  at: "2026-10-30T12:00:00Z", requestId: "request-123", feature: "suggestions",
  provider: "openai", model: "test-model", reason: "The AI provider returned an invalid meeting timestamp", details: null,
};

afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); });

describe("AI diagnostics", () => {
  it("shows device-local rejection reasons and request details, refreshes, opens logs, and clears", async () => {
    const read = vi.spyOn(mailClient, "aiDiagnostics").mockResolvedValue({ entries: [entry], captureUntil: null });
    const clear = vi.spyOn(mailClient, "clearAiDiagnostics").mockResolvedValue({ entries: [], captureUntil: null });
    const open = vi.spyOn(mailClient, "openLogsFolder").mockResolvedValue();
    render(<AiDiagnosticsSettings />);
    expect(await screen.findByText(entry.reason)).toBeInTheDocument();
    expect(screen.getByText(/test-model/)).toHaveTextContent("OpenAI");
    fireEvent.click(screen.getByText("Request details"));
    expect(screen.getByText("request-123")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Open Logs Folder" }));
    await waitFor(() => expect(open).toHaveBeenCalledOnce());
    await waitFor(() => expect(screen.getByRole("button", { name: "Refresh AI Diagnostics" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Refresh AI Diagnostics" }));
    await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.getByRole("button", { name: "Clear AI Diagnostics" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Clear AI Diagnostics" }));
    expect(await screen.findByText("No recent AI failures recorded on this device.")).toBeInTheDocument();
    expect(clear).toHaveBeenCalledOnce();
    expect(screen.queryByText(entry.reason)).not.toBeInTheDocument();
  });

  it("requires explicit capture, describes privacy and expiry, and expires the visible switch", async () => {
    vi.useFakeTimers();
    const until = new Date(Date.now() + 15 * 60_000).toISOString();
    vi.spyOn(mailClient, "aiDiagnostics").mockResolvedValue({ entries: [], captureUntil: null });
    const capture = vi.spyOn(mailClient, "setAiDiagnosticCapture").mockImplementation(async (enabled) => ({ entries: [], captureUntil: enabled ? until : null }));
    render(<AiDiagnosticsSettings />);
    await act(async () => {});
    const toggle = screen.getByRole("checkbox", { name: "Capture Detailed AI Diagnostics" });
    expect(toggle).not.toBeChecked();
    expect(toggle).toHaveAccessibleDescription(/Message bodies and evidence excerpts are never captured/);
    expect(toggle).toHaveAccessibleDescription(/Capture stops on restart/);
    expect(capture).not.toHaveBeenCalled();
    fireEvent.click(toggle);
    await act(async () => {});
    expect(capture).toHaveBeenCalledWith(true);
    expect(toggle).toBeChecked();
    fireEvent.click(toggle);
    await act(async () => {});
    expect(capture).toHaveBeenLastCalledWith(false);
    expect(toggle).not.toBeChecked();
    fireEvent.click(toggle);
    await act(async () => {});
    expect(toggle).toBeChecked();
    await act(async () => { vi.advanceTimersByTime(15 * 60_000); });
    expect(toggle).not.toBeChecked();
    expect(screen.queryByText(/Detailed capture ends/)).not.toBeInTheDocument();
  });

  it("shows scheduling values only when recorded and reports errors without losing entries", async () => {
    vi.spyOn(mailClient, "aiDiagnostics").mockResolvedValue({ entries: [{ ...entry, details: { normalizedStart: "bad timestamp" } }], captureUntil: null });
    vi.spyOn(mailClient, "openLogsFolder").mockRejectedValue(new Error("Folder unavailable"));
    render(<AiDiagnosticsSettings />);
    expect(await screen.findByText(entry.reason)).toBeInTheDocument();
    fireEvent.click(screen.getByText("Request details"));
    expect(screen.getByText("bad timestamp")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Open Logs Folder" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Folder unavailable");
    expect(screen.getByText(entry.reason)).toBeInTheDocument();
  });

  it("surfaces a failed initial read and lets the user retry", async () => {
    vi.spyOn(mailClient, "aiDiagnostics").mockRejectedValueOnce("Read failed").mockResolvedValue({ entries: [], captureUntil: null });
    render(<AiDiagnosticsSettings />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Read failed");
    fireEvent.click(screen.getByRole("button", { name: "Refresh AI Diagnostics" }));
    expect(await screen.findByText("No recent AI failures recorded on this device.")).toBeInTheDocument();
  });
});
