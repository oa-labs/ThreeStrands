import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { readFileSync } from "node:fs";
import postcss from "postcss";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Draft } from "./correspondence";
import { ThreadDrafts, threadDrafts } from "./ThreadDrafts";

afterEach(cleanup);

const draft = (overrides: Partial<Draft>): Draft => ({
  id: "d", revision: 1, account: "you@example.com", mode: "reply", sourceId: null, threadId: "t-1", replyId: "m@example.com",
  references: [], to: "jane@example.com", cc: "", bcc: "", subject: "Re: Plans", body: "", attachments: [], updatedAt: 0, ...overrides,
});

describe("threadDrafts", () => {
  it("matches drafts by account and conversation, not by subject", () => {
    const thread = { accountId: "you@example.com", providerThreadId: "t-1" };
    const mine = draft({ id: "mine" });
    const otherAccount = draft({ id: "other-account", account: "work@example.com" });
    const otherThread = draft({ id: "other-thread", threadId: "t-2" });
    const unthreaded = draft({ id: "new", mode: "new", threadId: null });
    expect(threadDrafts([mine, otherAccount, otherThread, unthreaded], thread).map((d) => d.id)).toEqual(["mine"]);
  });
});

describe("ThreadDrafts", () => {
  it("shows each unsent draft as a draft and resumes it", () => {
    const onOpen = vi.fn();
    const reply = draft({});
    render(<ThreadDrafts drafts={[reply, draft({ id: "empty", to: "" })]} onOpen={onOpen} />);
    const cards = screen.getAllByRole("group", { name: "Unsent draft" });
    expect(cards[0]).toHaveTextContent("Draft to jane@example.com");
    expect(cards[1]).toHaveTextContent("Draft with no recipients");
    fireEvent.click(screen.getAllByRole("button", { name: "Resume Draft" })[0]);
    expect(onOpen).toHaveBeenCalledWith(reply);
  });

  it("sizes the draft like a message card with Resume Draft centered at the right", () => {
    const css = postcss.parse(readFileSync("src/styles.css", "utf8"));
    const declarations = (selector: string) => {
      const values: Record<string, string> = {};
      css.walkRules((rule) => {
        if (!rule.selectors.includes(selector)) return;
        rule.walkDecls((declaration) => { values[declaration.prop] = declaration.value; });
      });
      return values;
    };
    const message = declarations(".message");
    const card = declarations(".thread-draft");
    expect(card["max-width"]).toBe(message["max-width"]);
    const horizontalPadding = (padding: string) => padding.split(" ")[1];
    expect(horizontalPadding(card.padding)).toBe(horizontalPadding(declarations(".message-card-expanded").padding));
    expect(card["justify-content"]).toBe("space-between");
    expect(card["align-items"]).toBe("center");
  });

  it("renders nothing for a conversation without drafts", () => {
    const { container } = render(<ThreadDrafts drafts={[]} onOpen={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });
});
