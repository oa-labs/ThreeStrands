/**
 * Manual order within a task board column, mirroring the backend's
 * `plan_board_ranks` so the demo client (and an optimistic board) place
 * tasks the way the app does.
 */

/** Ranks start this far apart, and a task placed at either end of a column lands this far past its neighbor. */
export const BOARD_RANK_STEP = 1024;
/** Below this gap between neighbors a midpoint is too close to tell apart, and the column is renumbered instead. */
export const MIN_BOARD_RANK_GAP = 1e-6;

export type BoardRankEntry = { id: string; boardRank?: number | null };

/**
 * The rank writes that make a column read in `order`, top to bottom, once `moved` is placed.
 * `order` lists each task with its current rank and includes `moved` once; its other tasks are already in the column.
 *
 * Unplaced tasks show above placed ones, so ranking only `moved` keeps the order just when every other task is
 * placed and already in order. Otherwise, or when its neighbors are too close to split, the column is renumbered
 * one step apart and only the ranks that change are returned.
 */
export function planBoardRanks(order: readonly BoardRankEntry[], moved: string): Map<string, number> {
  const index = order.findIndex((entry) => entry.id === moved);
  if (index === -1) return new Map();
  const rankOf = (entry: BoardRankEntry) => (typeof entry.boardRank === "number" && Number.isFinite(entry.boardRank) ? entry.boardRank : null);
  const others = order.filter((_, position) => position !== index).map(rankOf);
  const placed = others.every((rank): rank is number => rank !== null);
  if (placed && others.every((rank, position) => position === 0 || (others[position - 1] as number) < (rank as number))) {
    const previous = index > 0 ? (others[index - 1] as number) : null;
    const next = index < others.length ? (others[index] as number) : null;
    const fits = (rank: number) => (previous === null || previous < rank) && (next === null || rank < next);
    const current = rankOf(order[index]);
    if (current !== null && fits(current)) return new Map();
    let rank: number | null;
    if (previous !== null && next !== null) rank = next - previous > MIN_BOARD_RANK_GAP ? previous + (next - previous) / 2 : null;
    else if (previous !== null) rank = previous + BOARD_RANK_STEP;
    else if (next !== null) rank = next - BOARD_RANK_STEP;
    else rank = BOARD_RANK_STEP;
    if (rank !== null && Number.isFinite(rank) && fits(rank)) return new Map([[moved, rank]]);
  }
  const updates = new Map<string, number>();
  order.forEach((entry, position) => {
    const wanted = (position + 1) * BOARD_RANK_STEP;
    if (rankOf(entry) !== wanted) updates.set(entry.id, wanted);
  });
  return updates;
}
