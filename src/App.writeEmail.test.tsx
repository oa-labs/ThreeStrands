import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { App } from "./App";
import { mailClient } from "./data/client";
import { DEMO_ACCOUNT_ID } from "./data/demoDataset";
import type { ThreadTask } from "./domain";

afterEach(() => { cleanup(); vi.restoreAllMocks(); localStorage.clear(); });

const task = (overrides: Partial<ThreadTask>): ThreadTask => ({
  id: "t1", accountId: DEMO_ACCOUNT_ID, threadId: null, sourceMessageId: null, subjectSnapshot: null, title: "Send the contract",
  kind: "action", dueKind: "none", status: "open", createdAt: "2026-09-19T10:00:00Z", updatedAt: "2026-09-19T10:00:00Z", ...overrides,
});

async function writeEmailFor(title: string) {
  render(<App />);
  await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
  fireEvent.keyDown(window, { key: "0" });
  await screen.findByRole("region", { name: "Today" });
  fireEvent.click(await screen.findByRole("button", { name: `Actions for ${title}` }));
  fireEvent.click(screen.getByRole("menuitem", { name: "Write Email" }));
}

it("opens a new message titled for a standalone task and linked to it", async () => {
  localStorage.removeItem("threestrands.demoCorrespondence");
  vi.spyOn(mailClient, "listTasks").mockResolvedValue([task({})]);
  await writeEmailFor("Send the contract");
  const composer = await screen.findByRole("dialog", { name: "New Message" });
  expect(within(composer).getByRole("textbox", { name: "Subject" })).toHaveValue("Send the contract");
  expect(await within(composer).findByText(/^Completes:/)).toHaveTextContent("Completes: Send the contract");
});

it("opens a plain reply to the conversation's newest message for a mail task, linked to it", async () => {
  localStorage.removeItem("threestrands.demoCorrespondence");
  vi.spyOn(mailClient, "listTasks").mockResolvedValue([task({ title: "Answer the welcome", threadId: "welcome" })]);
  await writeEmailFor("Answer the welcome");
  const composer = await screen.findByRole("dialog", { name: "Reply Message" });
  expect(await within(composer).findByText(/^Completes:/)).toHaveTextContent("Completes: Answer the welcome");
});
