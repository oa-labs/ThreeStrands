import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AI_ACTIVITY_LIMIT, AiActivityList, aiActivityFeatureLabel } from "./AiActivityList";
import { mailClient } from "./data/client";
import type { AiActivity } from "./domain";

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
    expect(within(section).getByText(/Anthropic claude-sonnet-5 · 3 messages · 1\.3K tokens$/)).toBeInTheDocument();
    // A request that sent no messages doesn't claim a count.
    expect(within(section).getByText("Draft Review")).toBeInTheDocument();
    expect(within(section).getByText(/Custom Endpoint local · 1K tokens$/)).toBeInTheDocument();
    expect(section.querySelector("pre")).toBeNull();
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
