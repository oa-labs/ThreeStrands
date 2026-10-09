import { useEffect, useState } from "react";
import type { AiUsageDay } from "./domain";
import { mailClient } from "./data/client";
import { aiPriceKey, readAiPrices, saveAiPrice, type AiModelPrice, type AiProvider } from "./aiSettings";
import { aiUsageTotals, formatTokens, formatUsd, localDay } from "./aiUsage";
import { errorMessage } from "./errors";
import { plural } from "./plural";

/** Days of usage shown alongside today's total. */
export const AI_USAGE_WINDOW_DAYS = 7;

function priceInputValue(value: number | undefined): string {
  return value === undefined ? "" : String(value);
}

/**
 * Today's and the last week's provider usage, with an estimated cost so the
 * user can judge features such as proactive suggestions. Prices for each
 * configured model (the main model and, when set, the fast model) are entered
 * here unless the provider reports its own cost.
 */
export function AiUsageSummary({ provider, model, fastModel = "" }: { provider: AiProvider; model: string; fastModel?: string }) {
  const [rows, setRows] = useState<AiUsageDay[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [prices, setPrices] = useState(readAiPrices);
  const models = [...new Set([model, fastModel.trim()].filter(Boolean))];

  useEffect(() => {
    let active = true;
    mailClient.aiUsageSummary(AI_USAGE_WINDOW_DAYS)
      .then((result) => { if (active) setRows(result); })
      .catch((reason: unknown) => { if (active) setError(errorMessage(reason)); });
    return () => { active = false; };
  }, []);

  const today = localDay(new Date());
  const todayTotals = rows ? aiUsageTotals(rows.filter((row) => row.day === today), prices) : null;
  const weekTotals = rows ? aiUsageTotals(rows, prices) : null;
  const describe = (totals: NonNullable<typeof todayTotals>) =>
    `${plural(totals.requests, "request")} · ${formatTokens(totals.inputTokens + totals.outputTokens)} tokens · ${formatUsd(totals.costUsd)}${totals.unpricedRequests > 0 ? "+" : ""}`;

  return <section className="ai-usage" aria-label="AI usage">
    <h3>Usage</h3>
    {error ? <p className="form-error" role="alert">{error}</p> : null}
    {todayTotals && weekTotals ? <dl className="ai-usage-totals">
      <div><dt>Today</dt><dd>{describe(todayTotals)}</dd></div>
      <div><dt>Last {AI_USAGE_WINDOW_DAYS} days</dt><dd>{describe(weekTotals)}</dd></div>
    </dl> : !error ? <p className="settings-hint">Loading usage…</p> : null}
    {weekTotals && weekTotals.unpricedRequests > 0 ? <p className="settings-hint">
      {plural(weekTotals.unpricedRequests, "request has", "requests have")} no price yet, so the cost shown is a lower bound. Enter your model&rsquo;s prices to estimate it.
    </p> : null}
    {provider === "openrouter" ? <p className="settings-hint">OpenRouter reports the exact cost of each request.</p> : provider !== "none" ? models.map((name) =>
      <ModelPrices key={name} provider={provider} model={name} onSaved={() => setPrices(readAiPrices())} />) : null}
  </section>;
}

/** Per-million-token price inputs for one model, saved as they are typed. */
function ModelPrices({ provider, model, onSaved }: { provider: AiProvider; model: string; onSaved: () => void }) {
  const key = aiPriceKey(provider, model);
  const saved: AiModelPrice | undefined = readAiPrices()[key];
  const [inputPrice, setInputPrice] = useState(priceInputValue(saved?.inputPerMillion));
  const [outputPrice, setOutputPrice] = useState(priceInputValue(saved?.outputPerMillion));

  useEffect(() => {
    const current = readAiPrices()[key];
    setInputPrice(priceInputValue(current?.inputPerMillion));
    setOutputPrice(priceInputValue(current?.outputPerMillion));
  }, [key]);

  const commitPrice = (nextInput: string, nextOutput: string) => {
    const parse = (value: string) => value.trim() === "" ? null : Number(value);
    const input = parse(nextInput);
    const output = parse(nextOutput);
    const valid = (value: number | null) => value === null || (Number.isFinite(value) && value >= 0);
    if (!valid(input) || !valid(output)) return;
    saveAiPrice(provider, model, input === null && output === null ? null : { inputPerMillion: input ?? 0, outputPerMillion: output ?? 0 });
    onSaved();
  };

  return <div role="group" aria-label={`Prices for ${model}`}>
    <p className="settings-hint">Prices for {model}, in US dollars per million tokens. Check your provider&rsquo;s pricing page; they stay on this device.</p>
    <div className="ai-usage-prices">
      <label>Input price
        <input type="number" inputMode="decimal" min={0} step="any" value={inputPrice} onChange={(event) => { setInputPrice(event.target.value); commitPrice(event.target.value, outputPrice); }} />
      </label>
      <label>Output price
        <input type="number" inputMode="decimal" min={0} step="any" value={outputPrice} onChange={(event) => { setOutputPrice(event.target.value); commitPrice(inputPrice, event.target.value); }} />
      </label>
    </div>
  </div>;
}
