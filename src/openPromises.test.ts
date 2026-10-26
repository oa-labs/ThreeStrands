import { afterEach, describe, expect, it } from "vitest";
import type { OpenPromise } from "./domain";
import { dismissPromise, MAX_DISMISSED_PROMISES, promiseRecipients, readDismissedPromises } from "./openPromises";
import { todayThread } from "./test/todayMail";

const promise = (recipients: string[]): OpenPromise =>
  ({ thread: todayThread("t"), messageId: "m", sentAt: "2026-10-22T10:00:00Z", recipients, excerpt: "I'll send it" });

describe("open promises", () => {
  afterEach(() => localStorage.clear());

  it("names recipients by name where the To line gives one", () => {
    expect(promiseRecipients(promise(["Jill Hart <jill@example.org>"]))).toBe("Jill Hart");
    expect(promiseRecipients(promise(["Jill <jill@example.org>", "pete@example.org"]))).toBe("Jill and pete@example.org");
  });

  it("remembers dismissals on this device, forgetting the oldest past the limit", () => {
    expect(readDismissedPromises()).toEqual([]);
    for (let index = 0; index < MAX_DISMISSED_PROMISES; index += 1) dismissPromise(`m${index}`);
    expect(readDismissedPromises()).toHaveLength(MAX_DISMISSED_PROMISES);
    expect(readDismissedPromises()).toContain("m0");
    dismissPromise(`m${MAX_DISMISSED_PROMISES}`);
    expect(readDismissedPromises()).toHaveLength(MAX_DISMISSED_PROMISES);
    expect(readDismissedPromises()).not.toContain("m0");
  });

  it("reads a damaged entry as nothing dismissed", () => {
    localStorage.setItem("threestrands.today.dismissedPromises", "{not json");
    expect(readDismissedPromises()).toEqual([]);
  });
});
