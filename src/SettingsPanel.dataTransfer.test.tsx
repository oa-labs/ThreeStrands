import { readFileSync } from "node:fs";
import path from "node:path";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./userPreferences", () => ({
  exportSettings: vi.fn(),
  previewSettingsImport: vi.fn(),
  applySettingsImport: vi.fn(),
  cancelSettingsImport: vi.fn(),
}));

import {
  applySettingsImport,
  cancelSettingsImport,
  exportSettings,
  previewSettingsImport,
  type SettingsImportResult,
} from "./userPreferences";
import { DataTransferSettings, TRANSFERRED_DATA } from "./DataTransferSettings";

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

  it("names everything the encrypted export carries, as listed by the native payload", () => {
    const source = readFileSync(path.resolve("src-tauri/src/transfer.rs"), "utf8");
    const struct = /struct TransferPayload \{([\s\S]*?)\n\}/.exec(source)?.[1];
    expect(struct).toBeDefined();
    const fields = [...struct!.matchAll(/^\s*([a-z_]+):/gm)].map((match) => match[1]).filter((field) => field !== "version" && field !== "exported_at");
    expect(TRANSFERRED_DATA.map((item) => item.field).sort()).toEqual([...fields].sort());

    render(<DataTransferSettings onImported={vi.fn()} />);
    const hint = screen.getByText(/Creates a password-encrypted file/);
    expect(hint).toHaveTextContent(
      "containing your preferences, account list, Split Inboxes, signatures and their defaults, snippets, saved contacts, contact groups, muted senders, and retention setting.",
    );
    expect(hint).toHaveTextContent("Mail, tasks, OAuth credentials, passwords, API keys, and other keychain secrets are never exported.");
  });

  it("says import replaces local data but keeps accounts and asks to sign in again, for any provider", () => {
    render(<DataTransferSettings onImported={vi.fn()} />);
    const hint = screen.getByText(/Importing replaces/);
    expect(hint).toHaveTextContent(
      "Importing replaces the preferences, Split Inboxes, signatures and their defaults, snippets, saved contacts, contact groups, muted senders, and retention setting on this installation.",
    );
    expect(hint).toHaveTextContent("until you sign in to them again");
    expect(hint).not.toHaveTextContent(/Google/);
  });

  it("reports an import failure as an error, not a hint", async () => {
    vi.mocked(previewSettingsImport).mockRejectedValue(new Error("Wrong password or damaged file"));
    render(<DataTransferSettings onImported={vi.fn()} />);

    fireEvent.change(screen.getByLabelText("Backup File Password"), { target: { value: "correct horse" } });
    fireEvent.click(screen.getByRole("button", { name: "Choose Encrypted Settings File" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Wrong password or damaged file");
    expect(alert).toHaveClass("form-error");
  });

  function chooseImportFile() {
    fireEvent.change(screen.getByLabelText("Backup File Password"), { target: { value: "correct horse" } });
    fireEvent.click(screen.getByRole("button", { name: "Choose Encrypted Settings File" }));
  }

  const importResult = { accountCount: 3 } as SettingsImportResult;

  it("lets the user leave imported accounts off this device", async () => {
    vi.mocked(previewSettingsImport).mockResolvedValue({
      accounts: [
        { email: "work@example.com", displayName: null, nickname: "Work", provider: "gmail", setUpHere: true },
        { email: "personal@example.com", displayName: null, nickname: "Personal", provider: "imap", setUpHere: false },
        { email: "side@example.com", displayName: null, nickname: null, provider: "gmail", setUpHere: false },
      ],
    });
    vi.mocked(applySettingsImport).mockResolvedValue(importResult);
    const onImported = vi.fn();
    render(<DataTransferSettings onImported={onImported} />);

    chooseImportFile();
    const choices = await screen.findByRole("group", { name: "Accounts to Set Up on This Device" });
    expect(applySettingsImport).not.toHaveBeenCalled();
    const work = screen.getByRole("checkbox", { name: /Work \(work@example.com\)/ });
    expect(work).toBeChecked();
    expect(work).toBeDisabled();
    expect(choices).toHaveTextContent("Already set up here");
    const personal = screen.getByRole("checkbox", { name: /Personal \(personal@example.com\) · IMAP/ });
    expect(personal).toBeChecked();

    fireEvent.click(personal);
    fireEvent.click(screen.getByRole("button", { name: "Import" }));

    await vi.waitFor(() => expect(onImported).toHaveBeenCalledWith(importResult));
    expect(applySettingsImport).toHaveBeenCalledWith(["personal@example.com"]);
    expect(screen.queryByRole("group", { name: "Accounts to Set Up on This Device" })).not.toBeInTheDocument();
  });

  it("imports without asking when every account is already set up here", async () => {
    vi.mocked(previewSettingsImport).mockResolvedValue({
      accounts: [{ email: "work@example.com", displayName: null, nickname: null, provider: "gmail", setUpHere: true }],
    });
    vi.mocked(applySettingsImport).mockResolvedValue(importResult);
    const onImported = vi.fn();
    render(<DataTransferSettings onImported={onImported} />);

    chooseImportFile();

    await vi.waitFor(() => expect(onImported).toHaveBeenCalledWith(importResult));
    expect(applySettingsImport).toHaveBeenCalledWith([]);
    expect(screen.queryByRole("group")).not.toBeInTheDocument();
  });

  it("discards the chosen file when the account choice is cancelled", async () => {
    vi.mocked(previewSettingsImport).mockResolvedValue({
      accounts: [{ email: "personal@example.com", displayName: null, nickname: null, provider: "imap", setUpHere: false }],
    });
    vi.mocked(cancelSettingsImport).mockResolvedValue();
    render(<DataTransferSettings onImported={vi.fn()} />);

    chooseImportFile();
    await screen.findByRole("group", { name: "Accounts to Set Up on This Device" });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

    expect(cancelSettingsImport).toHaveBeenCalled();
    expect(applySettingsImport).not.toHaveBeenCalled();
    expect(screen.queryByRole("group", { name: "Accounts to Set Up on This Device" })).not.toBeInTheDocument();
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
