import { describe, expect, it } from "vitest";
import { planBoardRanks, type BoardRankEntry } from "./taskBoardRank";

const entries = (...pairs: [string, number | null][]): BoardRankEntry[] => pairs.map(([id, boardRank]) => ({ id, boardRank }));
const plan = (order: BoardRankEntry[], moved: string) => Object.fromEntries(planBoardRanks(order, moved));

describe("planBoardRanks", () => {
  it("numbers every task on a first placement in an unplaced column", () => {
    expect(plan(entries(["a", null], ["m", null], ["b", null]), "m")).toEqual({ a: 1024, m: 2048, b: 3072 });
    expect(plan(entries(["m", null]), "m")).toEqual({ m: 1024 });
  });

  it("ranks only the moved task among placed tasks", () => {
    expect(plan(entries(["m", 4096], ["a", 1024], ["b", 2048]), "m")).toEqual({ m: 0 });
    expect(plan(entries(["a", 1024], ["b", 2048], ["m", 0]), "m")).toEqual({ m: 3072 });
    expect(plan(entries(["a", 1024], ["m", 4096], ["b", 2048]), "m")).toEqual({ m: 1536 });
    expect(plan(entries(["a", 1024], ["m", null], ["b", 2048]), "m")).toEqual({ m: 1536 });
  });

  it("keeps the rank of a task already in place", () => {
    expect(plan(entries(["a", 1024], ["m", 1100], ["b", 2048]), "m")).toEqual({});
  });

  it("renumbers when neighbors are too close to split", () => {
    expect(plan(entries(["a", 1024], ["m", 9000], ["b", 1024 + 1e-7]), "m")).toEqual({ m: 2048, b: 3072 });
  });

  it("renumbers a column with unplaced or out-of-order tasks", () => {
    expect(plan(entries(["u", null], ["m", null], ["p", 1024]), "m")).toEqual({ u: 1024, m: 2048, p: 3072 });
    expect(plan(entries(["p", 1024], ["m", null], ["u", null]), "m")).toEqual({ m: 2048, u: 3072 });
    expect(plan(entries(["b", 2048], ["m", null], ["a", 1024]), "m")).toEqual({ b: 1024, m: 2048, a: 3072 });
  });

  it("plans nothing for a task missing from the order", () => {
    expect(plan(entries(["a", 1024]), "m")).toEqual({});
  });
});
