import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { mailClient } from "./data/client";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(() => Promise.resolve()) }));

describe("Inbox Assistants wiring", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it("opens Agents with shortcut 7, runs an assistant, surfaces suggestions in Mail context, and opens a reply draft for review", async () => {
    const createTask = vi.spyOn(mailClient, "createTask");
    render(<App />);

    // Open Agents workspace via shortcut 7
    await screen.findByRole("option", { name: /Welcome to ThreeStrands/ });
    fireEvent.keyDown(window, { key: "7" });
    const workspace = await screen.findByRole("region", { name: "Inbox Assistants" });

    // Run the Aging Reply Drafter directly from Agents
    const agingCard = within(workspace).getByRole("article", { name: "Aging Reply Drafter" });
    fireEvent.click(within(agingCard).getByRole("button", { name: "Run Now" }));

    // A reply suggestion card appears in the queue
    const welcomeSuggestion = await within(workspace).findByRole("article", { name: "Welcome to ThreeStrands" });
    expect(within(welcomeSuggestion).getByRole("button", { name: "Review Draft" })).toBeInTheDocument();

    // Jump back to Mail and verify the context panel shows the active suggestion for the selected thread
    fireEvent.click(screen.getByRole("button", { name: "Inbox (1)" }));
    fireEvent.click(await screen.findByRole("option", { name: /Welcome to ThreeStrands/ }));
    const banner = await screen.findByRole("region", { name: "Inbox Assistant Suggestions" });
    fireEvent.click(within(banner).getByRole("button", { name: "Review Draft" }));

    // Draft opens for review in compose without creating personal tasks
    const composer = await screen.findByRole("dialog", { name: "Reply Message" });
    expect(composer.querySelector('[aria-label="Message Body"]')?.textContent).toContain("Welcome to ThreeStrands");
    await waitFor(() => expect(createTask).not.toHaveBeenCalled());
  });
});
