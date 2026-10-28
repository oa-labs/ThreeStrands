import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { PrivacySettings } from "./PrivacySettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("local mail retention settings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  });

  it.each([30, 90, 365, null])("displays the saved retention choice %s", async (days) => {
    vi.mocked(invoke).mockResolvedValue(days);
    render(<PrivacySettings loadRemoteImages={false} onLoadRemoteImagesChange={vi.fn()} />);
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Keep Mail on This Device For" }))
      .toHaveValue(days === null ? "forever" : String(days)));
    expect(invoke).toHaveBeenCalledWith("get_retention_days");
  });

  it("can switch a new installation from 90 days to Forever", async () => {
    vi.mocked(invoke).mockResolvedValue(90);
    render(<PrivacySettings loadRemoteImages={false} onLoadRemoteImagesChange={vi.fn()} />);
    const retention = screen.getByRole("combobox", { name: "Keep Mail on This Device For" });
    await waitFor(() => expect(retention).toHaveValue("90"));
    fireEvent.change(retention, { target: { value: "forever" } });
    expect(invoke).toHaveBeenCalledWith("set_retention_days", { days: null });
    expect(retention).toHaveValue("forever");
  });

  it("explains Trash's starred exception to retention", async () => {
    vi.mocked(invoke).mockResolvedValue(90);
    render(<PrivacySettings loadRemoteImages={false} onLoadRemoteImagesChange={vi.fn()} />);
    await waitFor(() => expect(screen.getByRole("combobox")).toHaveValue("90"));
    const hint = screen.getByText(/Conversations whose newest message/);
    expect(hint).toHaveTextContent("Starred conversations are kept unless they are in Trash");
  });
});
