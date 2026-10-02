import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./userPreferences", () => ({ exportSettings: vi.fn(), importSettings: vi.fn() }));

import { exportSettings, importSettings } from "./userPreferences";
import { DataTransferSettings } from "./SettingsPanel";

describe("data transfer settings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  });
  afterEach(cleanup);

  function fillExportPasswords(password: string, confirmation = password) {
    fireEvent.change(screen.getByLabelText("Export Password"), { target: { value: password } });
    fireEvent.change(screen.getByLabelText("Confirm Password"), { target: { value: confirmation } });
  }

  it("reports an import failure as an error, not a hint", async () => {
    vi.mocked(importSettings).mockRejectedValue(new Error("Wrong password or damaged file"));
    render(<DataTransferSettings onImported={vi.fn()} />);

    fireEvent.change(screen.getByLabelText("Backup File Password"), { target: { value: "correct horse" } });
    fireEvent.click(screen.getByRole("button", { name: "Choose Encrypted Settings File" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Wrong password or damaged file");
    expect(alert).toHaveClass("form-error");
  });

  it("reports a successful export as a status", async () => {
    vi.mocked(exportSettings).mockResolvedValue("/tmp/threestrands.settings");
    render(<DataTransferSettings onImported={vi.fn()} />);

    fillExportPasswords("correct horse");
    fireEvent.click(screen.getByRole("button", { name: "Export Encrypted Settings" }));

    expect(await screen.findByRole("status")).toHaveTextContent("Settings exported to /tmp/threestrands.settings");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("explains why export is unavailable while the passwords are unusable", () => {
    render(<DataTransferSettings onImported={vi.fn()} />);
    const exportButton = screen.getByRole("button", { name: "Export Encrypted Settings" });

    fillExportPasswords("short");
    expect(screen.getByText("Use at least 8 characters.")).toBeInTheDocument();
    expect(exportButton).toBeDisabled();

    fillExportPasswords("correct horse", "correct hors");
    expect(screen.getByText("Passwords don’t match.")).toBeInTheDocument();
    expect(exportButton).toBeDisabled();

    fillExportPasswords("correct horse");
    expect(screen.queryByText("Passwords don’t match.")).not.toBeInTheDocument();
    expect(exportButton).toBeEnabled();
  });
});
