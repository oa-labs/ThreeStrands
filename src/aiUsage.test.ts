import { describe, expect, it } from "vitest";
import { aiUsageTotals, formatUsd, localDay } from "./aiUsage";
import type { AiUsageDay } from "./domain";

function row(overrides: Partial<AiUsageDay>): AiUsageDay {
  return {
    day: "2026-09-29", provider: "openai", model: "gpt-4o", requests: 1, inputTokens: 0, outputTokens: 0,
    reportedCostRequests: 0, reportedCostUsd: 0, ...overrides,
  };
}

describe("AI usage totals", () => {
  it("uses reported cost, then entered prices, and counts what has neither", () => {
    const totals = aiUsageTotals([
      row({ provider: "openrouter", model: "openai/gpt-4o", requests: 3, inputTokens: 900, outputTokens: 90, reportedCostRequests: 3, reportedCostUsd: 0.02 }),
      row({ requests: 2, inputTokens: 1_000_000, outputTokens: 500_000 }),
      row({ provider: "anthropic", model: "claude-sonnet-5", requests: 4, inputTokens: 10, outputTokens: 10 }),
      row({ provider: "openrouter", model: "other/model", requests: 2, reportedCostRequests: 1, reportedCostUsd: 0.5 }),
    ], { "openai:gpt-4o": { inputPerMillion: 2.5, outputPerMillion: 10 } });

    expect(totals.requests).toBe(11);
    expect(totals.inputTokens).toBe(1_000_910);
    expect(totals.outputTokens).toBe(500_100);
    expect(totals.costUsd).toBeCloseTo(0.02 + 2.5 + 5 + 0.5);
    expect(totals.unpricedRequests).toBe(5);
  });

  it("prices a partly reported row entirely from entered prices when they exist", () => {
    const totals = aiUsageTotals([row({ provider: "openrouter", model: "m", requests: 2, inputTokens: 1_000_000, reportedCostRequests: 1, reportedCostUsd: 9 })],
      { "openrouter:m": { inputPerMillion: 1, outputPerMillion: 1 } });
    expect(totals.costUsd).toBeCloseTo(1);
    expect(totals.unpricedRequests).toBe(0);
  });

  it("formats small costs and local days", () => {
    expect(formatUsd(0)).toBe("$0.00");
    expect(formatUsd(0.004)).toBe("<$0.01");
    expect(formatUsd(0.01)).toBe("$0.01");
    expect(formatUsd(12.345)).toBe("$12.35");
    expect(localDay(new Date(2026, 0, 5, 23, 59))).toBe("2026-01-05");
  });
});
