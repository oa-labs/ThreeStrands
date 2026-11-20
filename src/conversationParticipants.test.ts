import { expect, it } from "vitest";
import { conversationParticipants } from "./conversationParticipants";
import type { Message, ThreadDetail } from "./domain";

const message = (sender: string, recipients: string[]) => ({ sender, recipients } as Message);

it("lists each person on the conversation once, by lowercase address, without the user", () => {
  const detail = { thread: {}, messages: [
    message("Ada Lovelace <Ada@Example.com>", ["You <you@example.com>", "Grace <grace@example.com>, bob@example.com"]),
    message("you@example.com", ["Grace <grace@example.com>"]),
  ] } as unknown as ThreadDetail;
  expect(conversationParticipants(detail, new Set(["you@example.com"]))).toEqual([
    { email: "ada@example.com", name: "Ada Lovelace" }, { email: "grace@example.com", name: "Grace" }, { email: "bob@example.com", name: "bob@example.com" },
  ]);
  expect(conversationParticipants(null, new Set())).toEqual([]);
});
