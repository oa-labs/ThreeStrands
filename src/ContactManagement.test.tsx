import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import postcss from "postcss";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { ContactManagement, MergeContactsDialog } from "./ContactManagement";
import { mailClient } from "./data/client";
import type { ContactProfile, SaveContactRequest } from "./domain";

vi.mock("./data/client", () => ({ mailClient: {
  previewContactImport: vi.fn(), importContacts: vi.fn(), exportContacts: vi.fn(),
  listContactSuppressions: vi.fn(), setContactSuppressed: vi.fn(), mergeContacts: vi.fn(),
} }));
const request: SaveContactRequest = { id: null, displayName: "Jane", role: null, company: null, location: null, bio: null, notes: null, links: [], photoData: null, favorite: false, addresses: ["jane@example.com"], birthday: null };
const profile: ContactProfile = { ...request, id: "jane", sentCount: 0, receivedCount: 0, lastInteractedAt: null, keepInTouch: { intervalDays: null, startedAt: null, snoozedUntil: null, snoozedAt: null, lastTouchAt: null }, keepInTouchDueAt: null };
const other: ContactProfile = { ...profile, id: "other", displayName: "Other", addresses: ["other@example.com"] };
const open = () => fireEvent.click(screen.getByRole("button", { name: "Manage Contacts…" }));

describe("contact management", () => {
  beforeEach(() => vi.resetAllMocks());
  afterEach(cleanup);

  it("gives management actions a padded, stacked layout in a wider scrollable dialog", () => {
    render(<ContactManagement addresses={[]} onChanged={vi.fn()}/>); open();
    const dialog = screen.getByRole("dialog", { name: "Manage contacts" });
    const actions = within(dialog).getAllByRole("button").filter(button => button.getAttribute("aria-label") !== "Close");
    expect(actions).toHaveLength(3);
    expect(actions.every(button => button.closest(".modal-form")?.parentElement === dialog)).toBe(true);

    // jsdom does not lay out dialogs; guard against edge-flush actions and
    // clipped content using the rules applied to the rendered structure.
    const css = postcss.parse(readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8"));
    const declaration = (selector: string, property: string) => {
      let value: string | undefined;
      css.walkRules(selector, rule => {
        if (rule.selector === selector) rule.walkDecls(property, entry => { value = entry.value; });
      });
      return value;
    };
    expect(declaration(".contact-management-dialog", "width")).toBe("min(var(--dialog-w-lg), calc(100vw - var(--dialog-edge)))");
    expect(declaration(".contact-management-dialog", "overflow-y")).toBe("auto");
    expect(declaration(".modal-form", "padding")).toBe("var(--space-2) var(--dialog-inset) var(--space-4)");
    expect(declaration(".contact-management-dialog .modal-form", "padding-bottom")).toBe("var(--space-6)");
    expect(declaration(".contact-management-options", "display")).toBe("grid");
    expect(declaration(".contact-management-options .btn", "height")).toBe("auto");
    expect(declaration(".contact-management-options .btn", "white-space")).toBe("normal");

    fireEvent.click(within(dialog).getByRole("button", { name: "Export CSV or vCard…" }));
    expect(screen.getByLabelText("File format").closest(".modal-form")?.parentElement).toBe(dialog);
  });

  it("reviews warnings and contacts before importing and reports duplicate and invalid skips", async () => {
    vi.mocked(mailClient.previewContactImport).mockResolvedValue({ contacts: [request], skipped: 2, warnings: ["Phone fields omitted"] });
    vi.mocked(mailClient.importContacts).mockResolvedValue({ imported: 0, skipped: 1 });
    const changed = vi.fn();
    render(<ContactManagement addresses={[]} onChanged={changed}/>); open();
    fireEvent.click(screen.getByRole("button", { name: "Import CSV or vCard…" }));
    const dialog = await screen.findByRole("dialog", { name: "Review contact import" });
    expect(within(dialog).getByText("Phone fields omitted")).toBeVisible();
    expect(within(dialog).getByText(/Jane — jane@example.com/)).toBeInTheDocument();
    expect(mailClient.importContacts).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Import contacts" }));
    await screen.findByText("Imported 0 contacts. Skipped 3.");
    expect(mailClient.importContacts).toHaveBeenCalledExactlyOnceWith([request]);
    expect(changed).toHaveBeenCalledOnce();
  });

  it("leaves cancellation harmless and keeps a failed import review available for retry", async () => {
    vi.mocked(mailClient.previewContactImport).mockResolvedValueOnce(null).mockResolvedValueOnce({ contacts: [request], warnings: [], skipped: 0 });
    vi.mocked(mailClient.importContacts).mockRejectedValue(new Error("Import could not be saved"));
    const changed = vi.fn(); render(<ContactManagement addresses={[]} onChanged={changed}/>); open();
    fireEvent.click(screen.getByRole("button", { name: "Import CSV or vCard…" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Import CSV or vCard…" })).toBeEnabled());
    expect(mailClient.importContacts).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Import CSV or vCard…" }));
    fireEvent.click(await screen.findByRole("button", { name: "Import contacts" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Import could not be saved");
    expect(screen.getByRole("button", { name: "Import contacts" })).toBeEnabled(); expect(changed).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("exports the chosen format and keeps the dialog open if the file chooser is cancelled", async () => {
    vi.mocked(mailClient.exportContacts).mockResolvedValueOnce(false).mockResolvedValueOnce(true);
    render(<ContactManagement addresses={[]} onChanged={vi.fn()}/>); open();
    fireEvent.click(screen.getByRole("button", { name: "Export CSV or vCard…" }));
    expect(screen.getByText(/Contact files are unencrypted/)).toBeVisible();
    fireEvent.change(screen.getByLabelText("File format"), { target: { value: "vcard" } });
    fireEvent.click(screen.getByRole("button", { name: "Export contacts" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Export contacts" })).toBeEnabled());
    expect(mailClient.exportContacts).toHaveBeenCalledWith("vcard");
    fireEvent.click(screen.getByRole("button", { name: "Export contacts" }));
    await screen.findByText("Saved contacts exported."); expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("hides saved and arbitrary addresses and allows suggestions again", async () => {
    const suppressed = new Set<string>();
    vi.mocked(mailClient.listContactSuppressions).mockImplementation(async () => [...suppressed]);
    vi.mocked(mailClient.setContactSuppressed).mockImplementation(async (email, value) => { if (value) suppressed.add(email); else suppressed.delete(email); });
    render(<ContactManagement addresses={profile.addresses} onChanged={vi.fn()}/>); open();
    fireEvent.click(screen.getByRole("button", { name: "Manage recipient suggestions…" }));
    fireEvent.click(await screen.findByRole("button", { name: "Never suggest jane@example.com" }));
    fireEvent.click(await screen.findByRole("button", { name: "Allow suggestions for jane@example.com" }));
    await screen.findByText("No addresses are hidden.");
    fireEvent.change(screen.getByLabelText("Email address"), { target: { value: "unsaved@example.com" } });
    fireEvent.click(screen.getByRole("button", { name: "Never suggest this address" }));
    await screen.findByRole("button", { name: "Allow suggestions for unsaved@example.com" });
    expect(mailClient.setContactSuppressed).toHaveBeenCalledWith("jane@example.com", true);
    expect(mailClient.setContactSuppressed).toHaveBeenCalledWith("jane@example.com", false);
    expect(screen.getByText(/stays on this device/)).toBeVisible();
  });

  it("shows suppression failures without pretending that the address was hidden", async () => {
    vi.mocked(mailClient.listContactSuppressions).mockResolvedValue([]);
    vi.mocked(mailClient.setContactSuppressed).mockRejectedValue(new Error("Could not save preference"));
    render(<ContactManagement addresses={profile.addresses} onChanged={vi.fn()}/>); open();
    fireEvent.click(screen.getByRole("button", { name: "Manage recipient suggestions…" }));
    fireEvent.click(await screen.findByRole("button", { name: "Never suggest jane@example.com" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save preference");
    expect(screen.getByText("No addresses are hidden.")).toBeVisible();
  });

  it("merges only after choosing the surviving profile and keeps failures retryable", async () => {
    vi.mocked(mailClient.mergeContacts).mockRejectedValueOnce(new Error("Combined notes are too long")).mockResolvedValueOnce(other);
    const merged = vi.fn(); render(<MergeContactsDialog contacts={[profile, other]} onClose={vi.fn()} onMerged={merged}/>);
    expect(mailClient.mergeContacts).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("Profile to keep"), { target: { value: "other" } });
    fireEvent.click(screen.getByRole("button", { name: "Merge 2 contacts" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Combined notes are too long");
    expect(merged).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Merge 2 contacts" }));
    await waitFor(() => expect(merged).toHaveBeenCalledWith(other));
    expect(mailClient.mergeContacts).toHaveBeenLastCalledWith("other", ["jane"]);
  });
});
