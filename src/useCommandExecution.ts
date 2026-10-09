import { useCallback, useMemo, useRef, useState } from "react";
import {
  accountCommand,
  commands,
  showAllAccountsCommand,
  splitInboxCommand,
  undoResult,
  type Command,
  type CommandContext,
  type CommandResult,
} from "./commands";
import type { Account, SplitInbox } from "./domain";
import type { MutationTemplate } from "./threadMutations";
import type { Notice } from "./useNotice";
import { errorMessage } from "./errors";
import { useShortcutHandler } from "./useShortcutHandler";

export function useCommandUndo(setNotice: (notice: Notice | null) => void) {
  const lastUndo = useRef<{ command: Command; result: CommandResult } | null>(null);
  const [canUndoAction, setCanUndoAction] = useState(false);
  const undoLastAction = useCallback(async () => {
    const pending = lastUndo.current;
    if (!pending?.command.undo) return;
    lastUndo.current = null;
    setCanUndoAction(false);
    setNotice(null);
    try {
      await pending.command.undo(pending.result);
    } catch {
      setNotice({ message: "Undo could not be saved" });
    }
  }, [setNotice]);

  const rememberUndo = useCallback((command: Command, result: CommandResult) => {
    lastUndo.current = { command, result };
    setCanUndoAction(true);
    setNotice({ message: result.message ?? command.title, undo: () => { void undoLastAction(); } });
  }, [setNotice, undoLastAction]);
  return { canUndoAction, undoLastAction, rememberUndo };
}

type Options = {
  context: CommandContext;
  accounts: Account[];
  accountSplitInboxes: SplitInbox[];
  checkedIds: Set<string>;
  mutateIds: (ids: string[], template: MutationTemplate) => Promise<CommandResult>;
  rememberUndo: (command: Command, result: CommandResult) => void;
  setNotice: (notice: Notice | null) => void;
};

export function useCommandExecution({ context, accounts, accountSplitInboxes, checkedIds, mutateIds, rememberUndo, setNotice }: Options) {
  const executeCommand = useCallback((command: Command) => {
    void command.run(context)
      .then((result) => {
        if (!command.undo || !result.undoAction) return;
        rememberUndo(command, result);
      })
      .catch((error: unknown) => {
        setNotice({ message: errorMessage(error) });
      });
  }, [context, rememberUndo, setNotice]);
  const executeById = useCallback((id: string) => {
    const command = commands.find((candidate) => candidate.id === id);
    if (command?.enabled(context)) executeCommand(command);
  }, [context, executeCommand]);
  const accountCommands = useMemo<Command[]>(
    () =>
      accounts.length > 1
        ? [showAllAccountsCommand(), ...accounts.map((account, index) => accountCommand(account.email, index))]
        : [],
    [accounts],
  );
  const splitInboxCommands = useMemo<Command[]>(
    () => accountSplitInboxes.map((splitInbox) => splitInboxCommand(splitInbox)),
    [accountSplitInboxes],
  );
  const paletteExtraCommands = useMemo<Command[]>(
    () => [...accountCommands, ...splitInboxCommands],
    [accountCommands, splitInboxCommands],
  );
  useShortcutHandler(context, executeCommand, paletteExtraCommands);

  const runOnSelection = useCallback((title: string, template: MutationTemplate) => {
    executeCommand({
      id: "selection.batch",
      title,
      keys: [],
      group: "Triage",
      enabled: () => true,
      run: () => mutateIds([...checkedIds], template),
      undo: undoResult,
    });
  }, [checkedIds, executeCommand, mutateIds]);

  return { executeCommand, executeById, paletteExtraCommands, runOnSelection };
}
