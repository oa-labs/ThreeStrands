import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Draft } from "./correspondence";
import { mailClient } from "./data/client";
import { DraftsList, useDraftSelection } from "./DraftsList";

afterEach(async () => {
  cleanup();
  for (const draft of await mailClient.listDrafts()) await mailClient.discardDraft(draft.id);
});

function Harness({ drafts, onOpen = vi.fn() }: { drafts: Draft[]; onOpen?: (draft: Draft) => void }) {
  const selection = useDraftSelection(drafts, null);
  return <>
    <p>{selection.checkedIds.size} selected</p>
    <button type="button" onClick={selection.selectAll}>Select All Drafts</button>
    <div role="listbox" aria-label="Drafts">
      <DraftsList
        drafts={drafts}
        openId={null}
        checkedIds={selection.checkedIds}
        showAccount={false}
        accountColors={new Map()}
        onOpen={(draft) => { selection.open(draft.id); onOpen(draft); }}
        onDiscard={vi.fn()}
        onToggleCheck={selection.toggle}
        onSelectionGesture={selection.applyGesture}
      />
    </div>
  </>;
}

async function drafts(count: number) {
  for (let index = 0; index < count; index++) await mailClient.createDraft("new");
  return mailClient.listDrafts();
}

describe("drafts list selection", () => {
  it("checks every draft between two checkboxes when the second is Shift-clicked", async () => {
    render(<Harness drafts={await drafts(3)} />);
    const rows = screen.getAllByRole("option");
    fireEvent.click(rows[0].querySelector(".row-check")!);
    expect(screen.getByText("1 selected")).toBeInTheDocument();
    fireEvent.click(rows[2].querySelector(".row-check")!, { shiftKey: true });
    expect(screen.getByText("3 selected")).toBeInTheDocument();
    for (const row of rows) expect(row).toHaveTextContent("Selected for batch actions");
  });

  it("selects with Command-click, selects all, and clears the selection with Escape", async () => {
    const onOpen = vi.fn();
    render(<Harness drafts={await drafts(2)} onOpen={onOpen} />);
    fireEvent.click(screen.getAllByRole("option")[0], { metaKey: true });
    expect(screen.getByText("1 selected")).toBeInTheDocument();
    expect(onOpen).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Select All Drafts" }));
    expect(screen.getByText("2 selected")).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.getByText("0 selected")).toBeInTheDocument();
  });

  it("opens a draft with a plain click and ends the batch selection", async () => {
    const onOpen = vi.fn();
    render(<Harness drafts={await drafts(2)} onOpen={onOpen} />);
    const rows = screen.getAllByRole("option");
    fireEvent.click(rows[0], { metaKey: true });
    fireEvent.click(rows[1]);
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(screen.getByText("0 selected")).toBeInTheDocument();
  });
});
