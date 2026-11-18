import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AI_ACTIVITY_LIMIT, AiActivityList, aiActivityFeatureLabel } from "./AiActivityList";
import { mailClient } from "./data/client";
import type { AiActivity } from "./domain";
import { formatDateTime } from "./dateFormatting";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import postcss from "postcss";

function entry(overrides: Partial<AiActivity>): AiActivity {
  return {
    at: "2026-10-22T14:05:00Z",
    feature: "brief",
    provider: "anthropic",
    model: "claude-sonnet-5",
    subject: "Website setup",
    messageCount: 3,
    inputTokens: 1_200,
    outputTokens: 80,
    ...overrides,
  };
}

describe("AiActivityList", () => {
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it("lists what each request was for in the app's own words, never the email text", async () => {
    const activity = vi.spyOn(mailClient, "aiActivity").mockResolvedValue([
      entry({}),
      entry({ feature: "draft_review", provider: "custom", model: "local", subject: "Proposal", messageCount: null, inputTokens: 900, outputTokens: 100 }),
    ]);
    render(<AiActivityList />);
    const section = screen.getByRole("region", { name: "AI activity" });
    expect(await within(section).findByText("Brief")).toBeInTheDocument();
    expect(activity).toHaveBeenCalledWith(AI_ACTIVITY_LIMIT);
    expect(within(section).getByText("Website setup")).toBeInTheDocument();
    const table = within(section).getByRole("table", { name: "Recent AI requests" });
    expect(within(table).getAllByRole("columnheader").map((header) => header.textContent)).toEqual([
      "Activity / Conversation", "When", "Provider / Model", "Messages", "Tokens",
    ]);
    const rows = within(table).getAllByRole("row").slice(1);
    expect(within(rows[0]).getAllByRole("cell").map((cell) => cell.textContent)).toEqual([
      "BriefWebsite setup", formatDateTime(new Date("2026-10-22T14:05:00Z")), "Anthropicclaude-sonnet-5", "3", "1.3K",
    ]);
    expect(within(rows[0]).getByLabelText("3 messages")).toBeInTheDocument();
    expect(within(rows[0]).getByLabelText("1.3K tokens")).toBeInTheDocument();
    // A request that sent no messages doesn't claim a count.
    expect(within(section).getByText("Draft Review")).toBeInTheDocument();
    expect(within(rows[1]).getByText("Custom Endpoint")).toBeInTheDocument();
    expect(within(rows[1]).getByText("local")).toBeInTheDocument();
    expect(within(rows[1]).getByLabelText("Not applicable")).toHaveTextContent("—");
    expect(within(rows[1]).getByLabelText("1K tokens")).toBeInTheDocument();
    expect(section.querySelector("pre")).toBeNull();
  });

  it("keeps long subjects and model paths intact and handles requests without a conversation", async () => {
    const subject = "A long conversation subject ".repeat(12);
    const model = "accounts/fireworks/models/" + "long-model-name-".repeat(12);
    vi.spyOn(mailClient, "aiActivity").mockResolvedValue([
      entry({ subject, provider: "fireworks", model }),
      entry({ feature: "contact", subject: null, messageCount: 0, inputTokens: 0, outputTokens: 0 }),
    ]);
    render(<AiActivityList />);
    const table = await screen.findByRole("table", { name: "Recent AI requests" });
    const rows = within(table).getAllByRole("row").slice(1);
    expect(within(rows[0]).getByText(subject.trim())).toHaveTextContent(subject.trim());
    expect(within(rows[0]).getByText(model)).toBeInTheDocument();
    expect(within(rows[1]).getAllByRole("cell")[0]).toHaveTextContent(/^Enhance With AI$/);
    expect(within(rows[1]).getByLabelText("0 messages")).toHaveTextContent("0");
    expect(within(rows[1]).getByLabelText("0 tokens")).toHaveTextContent("0");
    expect(screen.getByRole("region", { name: "Activity table" })).toHaveAttribute("tabindex", "0");
  });

  it("wraps long activity details and contains narrow-width overflow within the table", () => {
    const css = postcss.parse(readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8"));
    const value = (selector: string, property: string) => {
      let result: string | undefined;
      css.walkRules(selector, (rule) => rule.walkDecls(property, (declaration) => { result = declaration.value; }));
      return result;
    };
    expect(value(".ai-activity", "min-width")).toBe("0");
    expect(value(".ai-activity-scroll", "overflow-x")).toBe("auto");
    expect(value(".ai-activity-table", "table-layout")).toBe("fixed");
    expect(value(".ai-activity-table th, .ai-activity-table td", "overflow-wrap")).toBe("anywhere");
    expect(value(".ai-activity-subject", "white-space")).not.toBe("nowrap");
    expect(value(".ai-activity-subject", "overflow")).not.toBe("hidden");
  });

  it("names every feature the native side tags, and unknown ones as Other", () => {
    for (const [feature, label] of [
      ["brief", "Brief"], ["summary", "Brief"], ["suggestions", "Suggestions"], ["chat", "Ask About This Conversation"],
      ["contact", "Enhance With AI"], ["reply", "Draft Reply"], ["draft_review", "Draft Review"], ["find_times", "Find Times"],
      ["other", "Other"], ["something-new", "Other"],
    ]) expect(aiActivityFeatureLabel(feature)).toBe(label);
  });

  it("shows the latest few until asked for more, and says when nothing was sent", async () => {
    vi.spyOn(mailClient, "aiActivity").mockResolvedValue(Array.from({ length: 12 }, (_, index) => entry({ subject: `Thread ${index}` })));
    const { unmount } = render(<AiActivityList />);
    expect(await screen.findByText("Thread 9")).toBeInTheDocument();
    expect(screen.queryByText("Thread 10")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show 2 More" }));
    expect(screen.getByText("Thread 11")).toBeInTheDocument();
    unmount();

    vi.spyOn(mailClient, "aiActivity").mockResolvedValue([]);
    render(<AiActivityList />);
    expect(await screen.findByText("Nothing has been sent yet.")).toBeInTheDocument();
  });
});
