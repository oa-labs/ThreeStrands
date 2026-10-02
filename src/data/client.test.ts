import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import type { MailClient } from "./client";

let client: MailClient;

beforeAll(async () => {
  // The native client is chosen when the module loads.
  Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  client = (await import("./client")).mailClient;
});

describe("native AI request payloads", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockResolvedValue({});
  });

  const sent = (command: string) => vi.mocked(invoke).mock.calls.find(([name]) => name === command)?.[1];

  it("forwards the reasoning switch for fast-tier work", async () => {
    await client.summarizeThread("thread-1", "fireworks", "fast", null, "off");
    expect(sent("ai_summarize_thread")).toEqual({ threadId: "thread-1", provider: "fireworks", model: "fast", endpoint: null, reasoning: "off" });

    const context = { subject: "Hello", messages: [] };
    await client.generateReply(context, "", "fireworks", "fast", null, "off");
    expect(sent("ai_generate_reply")).toEqual({ context, instruction: "", provider: "fireworks", model: "fast", endpoint: null, reasoning: "off" });

    await client.enrichContact("contact-1", "fireworks", "fast", null, ["role"], false, undefined, "off");
    expect(sent("ai_enrich_contact")).toMatchObject({ id: "contact-1", model: "fast", emptyFields: ["role"], reasoning: "off" });
  });

  it("asks for the model's default reasoning when the caller does not say", async () => {
    await client.summarizeThread("thread-1", "fireworks", "model", null);
    await client.generateReply({ subject: "Hello", messages: [] }, "", "fireworks", "model", null);
    await client.enrichContact("contact-1", "fireworks", "model", null, ["role"]);
    for (const command of ["ai_summarize_thread", "ai_generate_reply", "ai_enrich_contact"]) {
      expect(sent(command)).toMatchObject({ reasoning: "default" });
    }
  });
});
