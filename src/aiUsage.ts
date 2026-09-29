import type { AiUsageDay } from "./domain";
import { type AiModelPrice } from "./aiSettings";

export type AiUsageTotals = {
  requests: number;
  inputTokens: number;
  outputTokens: number;
  /** Reported cost where the provider gave one, otherwise estimated from entered prices. */
  costUsd: number;
  /** Requests with neither a reported cost nor an entered price. */
  unpricedRequests: number;
};

/** The local calendar day as `YYYY-MM-DD`, matching the stored usage days. */
export function localDay(date: Date): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/**
 * Totals usage rows. A row whose every request reported a cost uses that
 * cost; otherwise an entered price for its provider and model estimates the
 * whole row, and without one only the reported part is counted.
 */
export function aiUsageTotals(rows: AiUsageDay[], prices: Record<string, AiModelPrice>): AiUsageTotals {
  const totals: AiUsageTotals = { requests: 0, inputTokens: 0, outputTokens: 0, costUsd: 0, unpricedRequests: 0 };
  for (const row of rows) {
    totals.requests += row.requests;
    totals.inputTokens += row.inputTokens;
    totals.outputTokens += row.outputTokens;
    const price = prices[`${row.provider}:${row.model}`];
    if (row.requests > 0 && row.reportedCostRequests >= row.requests) {
      totals.costUsd += row.reportedCostUsd;
    } else if (price) {
      totals.costUsd += (row.inputTokens * price.inputPerMillion + row.outputTokens * price.outputPerMillion) / 1_000_000;
    } else {
      totals.costUsd += row.reportedCostUsd;
      totals.unpricedRequests += row.requests - row.reportedCostRequests;
    }
  }
  return totals;
}

export function formatUsd(value: number): string {
  if (value > 0 && value < 0.01) return "<$0.01";
  return new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" }).format(value);
}

export function formatTokens(value: number): string {
  return new Intl.NumberFormat(undefined, { notation: "compact", maximumFractionDigits: 1 }).format(value);
}
