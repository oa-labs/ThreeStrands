import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AiUsageSummary } from "./AiUsageSummary";
import { localDay } from "./aiUsage";
import { readAiPrices } from "./aiSettings";
import { mailClient } from "./data/client";
import type { AiUsageDay } from "./domain";

const now = new Date();
const today = localDay(now);
// Step back a calendar day, not 24 hours: on a 25-hour DST day, now − 24h
// can still be today.
const yesterday = localDay(new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1, 12));

function row(overrides: Partial<AiUsageDay>): AiUsageDay {
  return { day: today, provider: "openai", model: "gpt-4o", requests: 1, inputTokens: 0, outputTokens: 0, reportedCostRequests: 0, reportedCostUsd: 0, ...overrides };
}

describe("AiUsageSummary", () => {
  afterEach(() => { cleanup(); vi.restoreAllMocks(); localStorage.clear(); });

  it("shows today and the week, and estimates cost once prices are entered", async () => {
    const summary = vi.spyOn(mailClient, "aiUsageSummary").mockResolvedValue([
      row({ requests: 3, inputTokens: 1_500_000, outputTokens: 100_000 }),
      row({ day: yesterday, requests: 2, inputTokens: 500_000, outputTokens: 0 }),
    ]);
    render(<AiUsageSummary provider="openai" model="gpt-4o" />);

    const usage = screen.getByRole("region", { name: "AI usage" });
    expect(await within(usage).findByText(/^3 requests · 1\.6M tokens · \$0\.00\+$/)).toBeInTheDocument();
    expect(within(usage).getByText(/^5 requests · 2\.1M tokens · \$0\.00\+$/)).toBeInTheDocument();
    expect(within(usage).getByText(/5 requests have no price yet/)).toBeInTheDocument();
    expect(summary).toHaveBeenCalledWith(7);

    fireEvent.change(within(usage).getByLabelText("Input price"), { target: { value: "2" } });
    fireEvent.change(within(usage).getByLabelText("Output price"), { target: { value: "10" } });

    expect(within(usage).getByText("3 requests · 1.6M tokens · $4.00")).toBeInTheDocument();
    expect(within(usage).getByText("5 requests · 2.1M tokens · $5.00")).toBeInTheDocument();
    expect(within(usage).queryByText(/no price yet/)).not.toBeInTheDocument();
    expect(readAiPrices()).toEqual({ "openai:gpt-4o": { inputPerMillion: 2, outputPerMillion: 10 } });

    fireEvent.change(within(usage).getByLabelText("Input price"), { target: { value: "" } });
    fireEvent.change(within(usage).getByLabelText("Output price"), { target: { value: "" } });
    expect(readAiPrices()).toEqual({});
  });

  it("prices the fast model's requests separately from the main model's", async () => {
    vi.spyOn(mailClient, "aiUsageSummary").mockResolvedValue([
      row({ requests: 1, inputTokens: 1_000_000, outputTokens: 0 }),
      row({ model: "gpt-4o-mini", requests: 1, inputTokens: 1_000_000, outputTokens: 0 }),
    ]);
    render(<AiUsageSummary provider="openai" model="gpt-4o" fastModel=" gpt-4o-mini " />);
    const usage = screen.getByRole("region", { name: "AI usage" });
    expect(await within(usage).findByText(/2 requests have no price yet/)).toBeInTheDocument();

    const main = within(usage).getByRole("group", { name: "Prices for gpt-4o" });
    fireEvent.change(within(main).getByLabelText("Input price"), { target: { value: "2" } });
    expect(within(usage).getByText(/1 request has no price yet/)).toBeInTheDocument();

    const fast = within(usage).getByRole("group", { name: "Prices for gpt-4o-mini" });
    fireEvent.change(within(fast).getByLabelText("Input price"), { target: { value: "0.5" } });
    expect(within(usage).queryByText(/no price yet/)).not.toBeInTheDocument();
    expect(within(usage).getAllByText("2 requests · 2M tokens · $2.50")).toHaveLength(2);
    expect(readAiPrices()).toEqual({
      "openai:gpt-4o": { inputPerMillion: 2, outputPerMillion: 0 },
      "openai:gpt-4o-mini": { inputPerMillion: 0.5, outputPerMillion: 0 },
    });
  });

  it("asks for one model's prices when the fast model is blank or the same", async () => {
    vi.spyOn(mailClient, "aiUsageSummary").mockResolvedValue([]);
    render(<AiUsageSummary provider="openai" model="gpt-4o" fastModel="gpt-4o" />);
    await screen.findByText("Today");
    expect(screen.getAllByRole("group", { name: /^Prices for/ })).toHaveLength(1);
  });

  it("ignores an invalid price instead of saving it", async () => {
    vi.spyOn(mailClient, "aiUsageSummary").mockResolvedValue([]);
    render(<AiUsageSummary provider="anthropic" model="claude-sonnet-5" />);
    await screen.findByText("Today");
    fireEvent.change(screen.getByLabelText("Input price"), { target: { value: "-3" } });
    expect(readAiPrices()).toEqual({});
  });

  it("uses OpenRouter's reported cost without asking for prices", async () => {
    vi.spyOn(mailClient, "aiUsageSummary").mockResolvedValue([
      row({ provider: "openrouter", model: "openai/gpt-4o", requests: 2, inputTokens: 900, outputTokens: 100, reportedCostRequests: 2, reportedCostUsd: 0.25 }),
    ]);
    render(<AiUsageSummary provider="openrouter" model="openai/gpt-4o" />);

    expect(await screen.findAllByText("2 requests · 1K tokens · $0.25")).toHaveLength(2);
    expect(screen.getByText("OpenRouter reports the exact cost of each request.")).toBeInTheDocument();
    expect(screen.queryByLabelText("Input price")).not.toBeInTheDocument();
  });

  it("reports a failure to load usage", async () => {
    vi.spyOn(mailClient, "aiUsageSummary").mockRejectedValue(new Error("Database is locked"));
    render(<AiUsageSummary provider="openai" model="gpt-4o" />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Database is locked");
  });
});
