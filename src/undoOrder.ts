let order = 0;

/** Orders undoable actions even when several finish within one clock tick. */
export const nextUndoOrder = () => ++order;
