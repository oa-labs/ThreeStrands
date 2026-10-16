import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

/**
 * The sparkles icon means "this click sends something to your AI provider".
 * Buttons that only open, preview, or set up AI carry no mark, and a retry or
 * refresh repeats a request the user already started from a marked button.
 */

const source = (file: string) => readFileSync(resolve(process.cwd(), "src", file), "utf8");

/** Client methods that send mail, drafts, or questions to the AI provider. */
const AI_CLIENT_METHODS = [
  "summarizeThread", "analyzeThread", "briefThread", "threadChat",
  "reviewDraft", "generateReply", "enrichContact", "findTimes",
];

/** Where each AI request starts, and the text of the button that starts it. */
const AI_BUTTONS: { file: string; label: string }[] = [
  { file: "ThreadAssist.tsx", label: `"Get Brief" : "Get Suggestions"}` },
  { file: "ThreadChat.tsx", label: "Ask</button>" },
  { file: "Composer.tsx", label: "Draft reply" },
  { file: "Composer.tsx", label: `"Generate Draft"` },
  { file: "DraftReviewSection.tsx", label: `"Get Feedback"` },
  { file: "ContactsWorkspace.tsx", label: "Enhance With AI" },
  { file: "RecipientSections.tsx", label: `"Find Times"` },
  { file: "FindTimesCard.tsx", label: "Adjust</button>" },
];

/** Files that start AI requests; a new one must add its buttons above. */
const AI_CALL_SITES = [
  "useThreadIntelligence.ts", "Composer.tsx", "DraftReviewSection.tsx", "ContactsWorkspace.tsx", "FindTimesCard.tsx",
];

/** The `<button …>…</button>` element around `index`, or null when it isn't inside one. */
function enclosingButton(text: string, index: number): string | null {
  const start = text.lastIndexOf("<button", index);
  if (start === -1 || text.lastIndexOf("</button>", index) > start) return null;
  const end = text.indexOf("</button>", index);
  return end === -1 ? null : text.slice(start, end + "</button>".length);
}

describe("AI buttons", () => {
  it("lists every client method that reaches the AI provider", () => {
    // Any method taking a provider sends something to it.
    const methods = [...source("data/client.ts").matchAll(/^\s+(\w+)\([^)]*provider: AiProvider/gm)].map((match) => match[1]);
    expect(new Set(methods)).toEqual(new Set(AI_CLIENT_METHODS));
  });

  it("knows every file that starts an AI request, so new AI buttons get registered", () => {
    const callers = readdirSync(resolve(process.cwd(), "src"))
      .filter((file) => /\.tsx?$/.test(file) && !/\.test\.tsx?$/.test(file))
      .filter((file) => AI_CLIENT_METHODS.some((method) => source(file).includes(`mailClient.${method}(`)));
    expect(callers.sort()).toEqual([...AI_CALL_SITES].sort());
  });

  it.each(AI_BUTTONS)("marks the button that sends to the provider in $file: $label", ({ file, label }) => {
    const text = source(file);
    const buttons = [...text.matchAll(new RegExp(label.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "g"))]
      .map((match) => enclosingButton(text, match.index))
      .filter((button): button is string => button !== null);
    expect(buttons.length, `no <button> contains ${label}`).toBeGreaterThan(0);
    for (const button of buttons) expect(button).toContain("<Sparkles");
  });

  it("draws the sparkles mark in one color on every AI button, not per section", () => {
    const styles = source("styles.css");
    expect(styles).toContain(".btn:not(.btn-primary) > .lucide-sparkles { color:var(--accent); }");
    // A section-specific tint would let one AI button drift from the others.
    const tints = [...styles.matchAll(/^([^{}\n]*)\{[^}]*\bcolor:/gm)]
      .map((match) => match[1].trim())
      .filter((selector) => /sparkles|thread-assist-run|ai-run/.test(selector) && selector !== ".btn:not(.btn-primary) > .lucide-sparkles");
    expect(tints).toEqual([]);
  });
});
