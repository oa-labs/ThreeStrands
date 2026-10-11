import { useEffect, useRef } from "react";
import {
  commands,
  isEditableTarget,
  matchesShortcut,
  shortcutSteps,
  type Command,
  type CommandContext,
  type InteractionScope,
} from "./commands";
import { escapeDismissPending } from "./useEscapeDismiss";

function targetShortcutScope(target: EventTarget | null): InteractionScope | null {
  if (!(target instanceof HTMLElement)) return null;
  const value = target.closest<HTMLElement>("[data-shortcut-scope]")?.dataset.shortcutScope;
  return value === "read" || value === "compose" || value === "search" || value === "modal" || value === "palette"
    ? value
    : null;
}

function reservesNativeActivation(event: KeyboardEvent): boolean {
  if (!(event.target instanceof HTMLElement) || (event.key !== "Enter" && event.code !== "Space")) return false;
  return Boolean(event.target.closest("button, a[href], summary, [role='button'], [role='menuitem'], [role='menuitemcheckbox'], [role='option']"));
}

export function useShortcutHandler(
  context: CommandContext,
  execute: (command: Command) => void,
  extraCommands: Command[] = [],
) {
  const contextRef = useRef(context);
  contextRef.current = context;
  const executeRef = useRef(execute);
  executeRef.current = execute;
  const extraRef = useRef(extraCommands);
  extraRef.current = extraCommands;
  const pendingStep = useRef<string | null>(null);
  const pendingTimeout = useRef<number | null>(null);

  useEffect(() => {
    const clearPendingStep = () => {
      pendingStep.current = null;
      if (pendingTimeout.current !== null) {
        window.clearTimeout(pendingTimeout.current);
        pendingTimeout.current = null;
      }
    };

    const onKeyDown = (event: KeyboardEvent) => {
      const currentContext = contextRef.current;
      if (currentContext.closing || event.isComposing || event.defaultPrevented) return;
      // An open overlay closes on Escape; its listener may run after this one.
      if (event.key === "Escape" && escapeDismissPending()) return;
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        clearPendingStep();
        event.preventDefault();
        currentContext.openPalette();
        return;
      }
      const interactionScope = targetShortcutScope(event.target) ?? currentContext.interactionScope;
      const sendShortcut = event.target instanceof HTMLElement && Boolean(event.target.closest(".composer")) && currentContext.composerActive && (event.metaKey || event.ctrlKey) && event.key === "Enter";
      const replyAssistShortcut = event.target instanceof HTMLElement && Boolean(event.target.closest(".composer")) && currentContext.composerActive && (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "j";
      const scheduleShortcut = interactionScope === "compose" && event.target instanceof HTMLElement && Boolean(event.target.closest(".composer")) && currentContext.composerActive && matchesShortcut(event, "Mod+Shift+L");
      // In Drafts, "#" discards the open draft unless focus is somewhere "#" is text.
      const discardDraftShortcut = interactionScope === "compose" && currentContext.composerActive && currentContext.mailbox === "drafts" && !isEditableTarget(event.target) && matchesShortcut(event, "#");
      const fontShortcut = (event.metaKey || event.ctrlKey) && ["=", "+", "-"].includes(event.key);
      // Moves between the draft and the context panel, so it must work from inside the draft's fields.
      const contextPanelShortcut = currentContext.composerActive && (matchesShortcut(event, "F6") || matchesShortcut(event, "Mod+Shift+p"));
      const allowsMailboxTabShortcut = event.key === "Tab"
        && event.target instanceof HTMLElement
        && event.target.hasAttribute("data-mailbox-tab-shortcut");
      const focusedControl = event.key === "Tab"
        && event.target instanceof HTMLElement
        && event.target !== document.body
        && event.target.matches("button, a[href], [tabindex]");
      const entryScope = interactionScope !== "read";
      if (
        reservesNativeActivation(event)
        || (!sendShortcut && !replyAssistShortcut && !scheduleShortcut && !discardDraftShortcut && !fontShortcut && !contextPanelShortcut && !allowsMailboxTabShortcut
          && (entryScope || isEditableTarget(event.target) || focusedControl))
      ) {
        clearPendingStep();
        return;
      }

      const allCommands = [...commands, ...extraRef.current];
      // A held key only repeats commands that opt in. Anything else (Archive,
      // Trash, a chord step) still claims the repeat so it can't fall through.
      if (event.repeat) {
        const command = allCommands.find(
          (candidate) =>
            candidate.enabled(currentContext) &&
            candidate.keys.some((key) => {
              const steps = shortcutSteps(key);
              return steps.length === 1 && matchesShortcut(event, steps[0]);
            }),
        );
        if (command) event.preventDefault();
        if (command?.repeatable) executeRef.current(command);
        return;
      }
      if (pendingStep.current) {
        const command = allCommands.find(
          (candidate) =>
            candidate.enabled(currentContext) &&
            candidate.keys.some((key) => {
              const steps = shortcutSteps(key);
              return steps.length === 2 &&
                steps[0].toLocaleLowerCase() === pendingStep.current &&
                matchesShortcut(event, steps[1]);
            }),
        );
        clearPendingStep();
        if (command) {
          event.preventDefault();
          executeRef.current(command);
          return;
        }
      }

      const command = allCommands.find(
        (candidate) =>
          candidate.enabled(currentContext) &&
          candidate.keys.some((key) => {
            const steps = shortcutSteps(key);
            return steps.length === 1 && matchesShortcut(event, steps[0]);
          }),
      );
      if (command) {
        event.preventDefault();
        executeRef.current(command);
        return;
      }

      const prefix = allCommands
        .filter((candidate) => candidate.enabled(currentContext))
        .flatMap((candidate) => candidate.keys)
        .map(shortcutSteps)
        .find((steps) => steps.length === 2 && matchesShortcut(event, steps[0]));
      if (!prefix) return;
      event.preventDefault();
      pendingStep.current = prefix[0].toLocaleLowerCase();
      pendingTimeout.current = window.setTimeout(clearPendingStep, 1000);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      clearPendingStep();
    };
  }, []);
}
