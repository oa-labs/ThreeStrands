import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { openUrl } from "@tauri-apps/plugin-opener";

vi.mock("./appUpdate");
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

import * as appUpdate from "./appUpdate";
import { UPDATE_CHECK_INTERVAL_MS, UpdateNotice } from "./UpdateNotice";

const update = (overrides: Partial<appUpdate.AvailableUpdate> = {}): appUpdate.AvailableUpdate => ({
  version: "0.73.0",
  currentVersion: "0.72.0",
  releaseUrl: "https://github.com/oa-labs/ThreeStrands/releases/tag/v0.73.0",
  installMode: "inPlace",
  ...overrides,
});

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(openUrl).mockResolvedValue(undefined);
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("UpdateNotice", () => {
  it("stays hidden when this build is current", async () => {
    vi.mocked(appUpdate.checkForAppUpdate).mockResolvedValue(null);
    render(<UpdateNotice />);
    await waitFor(() => expect(appUpdate.checkForAppUpdate).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("stays hidden when the check fails", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.mocked(appUpdate.checkForAppUpdate).mockRejectedValue(new Error("offline"));
    render(<UpdateNotice />);
    await waitFor(() => expect(warn).toHaveBeenCalled());
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    warn.mockRestore();
  });

  it("installs only after the user chooses to", async () => {
    vi.mocked(appUpdate.checkForAppUpdate).mockResolvedValue(update());
    vi.mocked(appUpdate.installAppUpdate).mockReturnValue(new Promise(() => {}));
    render(<UpdateNotice />);

    expect(await screen.findByText("ThreeStrands 0.73.0 is available.")).toBeInTheDocument();
    expect(appUpdate.installAppUpdate).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Install and restart" }));
    expect(appUpdate.installAppUpdate).toHaveBeenCalledTimes(1);
    expect(await screen.findByText(/Installing ThreeStrands 0.73.0/)).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("opens the release notes", async () => {
    vi.mocked(appUpdate.checkForAppUpdate).mockResolvedValue(update());
    render(<UpdateNotice />);
    fireEvent.click(await screen.findByRole("button", { name: "What's new" }));
    expect(openUrl).toHaveBeenCalledWith("https://github.com/oa-labs/ThreeStrands/releases/tag/v0.73.0");
  });

  it("offers a download instead of installing for package-manager installs", async () => {
    vi.mocked(appUpdate.checkForAppUpdate).mockResolvedValue(update({ installMode: "download" }));
    render(<UpdateNotice />);

    fireEvent.click(await screen.findByRole("button", { name: "Download" }));
    expect(openUrl).toHaveBeenCalledWith("https://github.com/oa-labs/ThreeStrands/releases/tag/v0.73.0");
    expect(screen.queryByRole("button", { name: "Install and restart" })).not.toBeInTheDocument();
  });

  it("asks a Mac app running from its disk image to move to Applications", async () => {
    vi.mocked(appUpdate.checkForAppUpdate).mockResolvedValue(update({ installMode: "moveToApplications" }));
    render(<UpdateNotice />);

    expect(await screen.findByText(/Move ThreeStrands to your Applications folder/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Download" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Install and restart" })).not.toBeInTheDocument();
  });

  it("reports a failed install and lets the user retry or download", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.mocked(appUpdate.checkForAppUpdate).mockResolvedValue(update());
    vi.mocked(appUpdate.installAppUpdate).mockRejectedValueOnce("Unable to install the update: signature mismatch");
    render(<UpdateNotice />);

    fireEvent.click(await screen.findByRole("button", { name: "Install and restart" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("ThreeStrands 0.73.0 couldn't be installed.");
    expect(screen.getByRole("button", { name: "Download" })).toBeInTheDocument();

    vi.mocked(appUpdate.installAppUpdate).mockReturnValue(new Promise(() => {}));
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(appUpdate.installAppUpdate).toHaveBeenCalledTimes(2);
  });

  it("keeps a dismissed version hidden on later checks but shows a newer one", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.mocked(appUpdate.checkForAppUpdate).mockResolvedValue(update());
    render(<UpdateNotice />);

    fireEvent.click(await screen.findByRole("button", { name: "Dismiss update" }));
    expect(screen.queryByRole("status")).not.toBeInTheDocument();

    await act(async () => { await vi.advanceTimersByTimeAsync(UPDATE_CHECK_INTERVAL_MS); });
    expect(appUpdate.checkForAppUpdate).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("status")).not.toBeInTheDocument();

    vi.mocked(appUpdate.checkForAppUpdate).mockResolvedValue(update({ version: "0.74.0" }));
    await act(async () => { await vi.advanceTimersByTimeAsync(UPDATE_CHECK_INTERVAL_MS); });
    expect(await screen.findByText("ThreeStrands 0.74.0 is available.")).toBeInTheDocument();
  });
});
