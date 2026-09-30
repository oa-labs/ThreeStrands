import { describe, expect, it } from "vitest";
import type { MailClient } from "./client";
import { demoClient } from "./demoClient";

function providerContract(name: string, client: MailClient) {
  describe(`${name} provider contract`, () => {
    it("returns list rows that resolve to coherent details", async () => {
      const threads = await client.listThreads();
      expect(threads.length).toBeGreaterThan(0);
      const detail = await client.getThread(threads[0]!.id);
      expect(detail.thread.id).toBe(threads[0]!.id);
      expect(detail.messages.every((message) => message.threadId === detail.thread.id)).toBe(true);
    });

    it("searches and applies a reversible label mutation", async () => {
      const threads = await client.searchThreads({ query: "ThreeStrands" });
      expect(threads.length).toBeGreaterThan(0);
      const thread = threads[0]!;
      const label = await client.createLabel(`Contract ${crypto.randomUUID()}`);
      await client.mutateThread({
        kind: "label",
        threadId: thread.id,
        labelId: label.id,
        value: true,
      });
      expect((await client.getThread(thread.id)).thread.labels).toContain(label.id);
      await client.mutateThread({
        kind: "label",
        threadId: thread.id,
        labelId: label.id,
        value: false,
      });
      const restored = (await client.getThread(thread.id)).thread;
      expect(restored.labels).not.toContain(label.id);
      expect(restored.labels).toEqual(thread.labels);
      await client.deleteLabel(label.id);
    });

    it("summarizes a thread and persists the result", async () => {
      const threads = await client.listThreads();
      const thread = threads[0]!;
      const result = await client.summarizeThread(thread.id, "openai", "gpt-4o", null);
      expect(result.summary.length).toBeGreaterThan(0);
      const detail = await client.getThread(thread.id);
      expect(detail.thread.summary).toBe(result.summary);
      expect(detail.thread.summaryGeneratedAt).toBe(result.generatedAt);
    });
  });
}

providerContract("demo", demoClient);
