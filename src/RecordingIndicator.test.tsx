import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RecordingStatus } from "./domain";
import { RecordingIndicator } from "./RecordingIndicator";

const status = (overrides: Partial<RecordingStatus> = {}): RecordingStatus => ({
  state: "recording", recordingId: "r1", title: "Weekly Sync", eventRef: null, elapsedMs: 65_000, tracks: [],
  systemAudio: { kind: "available" }, lastStopReason: null, ...overrides,
});

describe("RecordingIndicator", () => {
  beforeEach(() => { vi.useFakeTimers(); vi.setSystemTime(new Date("2026-11-27T15:00:00Z")); });
  afterEach(() => { cleanup(); vi.useRealTimers(); });

  it("renders nothing while idle", () => {
    const { container } = render(<RecordingIndicator status={status({ state: "idle", recordingId: null })} onStop={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows the elapsed time and advances it locally while recording", () => {
    render(<RecordingIndicator status={status()} onStop={vi.fn()} />);
    expect(screen.getByRole("timer")).toHaveTextContent("1:05");
    act(() => { vi.advanceTimersByTime(3000); });
    expect(screen.getByRole("timer")).toHaveTextContent("1:08");
  });

  it("starts again from a newer state emitted by the recorder", () => {
    const { rerender } = render(<RecordingIndicator status={status()} onStop={vi.fn()} />);
    act(() => { vi.advanceTimersByTime(5000); });
    rerender(<RecordingIndicator status={status({ elapsedMs: 120_000 })} onStop={vi.fn()} />);
    expect(screen.getByRole("timer")).toHaveTextContent("2:00");
    act(() => { vi.advanceTimersByTime(1000); });
    expect(screen.getByRole("timer")).toHaveTextContent("2:01");
  });

  it("says Paused and holds the clock still", () => {
    render(<RecordingIndicator status={status({ state: "paused" })} onStop={vi.fn()} />);
    expect(screen.getByRole("timer")).toHaveTextContent("Paused");
    act(() => { vi.advanceTimersByTime(10_000); });
    expect(screen.getByRole("timer")).toHaveTextContent("Paused");
  });

  it("stops the recording from Stop", () => {
    const onStop = vi.fn();
    render(<RecordingIndicator status={status()} onStop={onStop} />);
    screen.getByRole("button", { name: "Stop Recording" }).click();
    expect(onStop).toHaveBeenCalledOnce();
  });

  it("never takes focus when it appears", () => {
    const outside = document.createElement("button");
    document.body.append(outside);
    outside.focus();
    const { rerender } = render(<RecordingIndicator status={status({ state: "idle" })} onStop={vi.fn()} />);
    rerender(<RecordingIndicator status={status()} onStop={vi.fn()} />);
    expect(screen.getByRole("group", { name: "Recording" })).toBeInTheDocument();
    expect(outside).toHaveFocus();
    outside.remove();
  });
});
