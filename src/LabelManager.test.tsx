import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { LabelManager } from "./LabelManager";
import type { Label } from "./domain";
import { labelCapabilities } from "./labels";

afterEach(cleanup);

const labels: Label[] = [
  { id: "lf:Clients", name: "Clients", kind: "user" },
  { id: "lf:Projects", name: "Projects", kind: "user" },
];

function renderManager(labelModel: Parameters<typeof labelCapabilities>[0]) {
  return render(
    <LabelManager
      labels={labels}
      accountId="imap@example.com"
      capabilities={labelCapabilities(labelModel)}
      checkedLabelIds={new Set()}
      onClose={() => {}}
      onCreate={vi.fn(async () => labels[0]!)}
      onDelete={vi.fn(async () => {})}
      onRename={vi.fn(async () => {})}
      onToggle={vi.fn()}
    />,
  );
}

describe("LabelManager capability gating", () => {
  it("offers create (via the picker), delete and rename for Gmail labels", () => {
    renderManager("gmailLabels");
    expect(screen.getByRole("combobox", { name: "Find or Create a Label" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Rename Clients" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete Clients" })).toBeInTheDocument();
  });

  it("hides rename but keeps create/delete for IMAP label folders", () => {
    renderManager("imapLabelFolders");
    // Create still available via the picker, delete still available…
    expect(screen.getByRole("combobox", { name: "Find or Create a Label" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete Clients" })).toBeInTheDocument();
    // …but rename can only fail and roll back, so it is not offered.
    expect(screen.queryByRole("button", { name: "Rename Clients" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Rename Projects" })).not.toBeInTheDocument();
  });

  it("hides all label controls and explains why for an IMAP account with no label storage", () => {
    renderManager("imapNoUserLabels");
    const dialog = screen.getByRole("dialog", { name: "Manage Labels" });
    expect(within(dialog).queryByRole("combobox", { name: "Find or Create a Label" })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Delete Clients" })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Rename Clients" })).not.toBeInTheDocument();
    // A short, provider-neutral explanation is shown instead.
    expect(within(dialog).getByText(/does not support labels/i)).toBeInTheDocument();
  });

  it("hides all label controls for an IMAP keyword account too", () => {
    renderManager("imapKeywords");
    const dialog = screen.getByRole("dialog", { name: "Manage Labels" });
    expect(within(dialog).queryByRole("combobox", { name: "Find or Create a Label" })).not.toBeInTheDocument();
    expect(within(dialog).getByText(/does not support labels/i)).toBeInTheDocument();
  });
});
