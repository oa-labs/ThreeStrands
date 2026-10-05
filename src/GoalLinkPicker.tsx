import { Check } from "lucide-react";
import { useState } from "react";
import { Modal } from "./AppChrome";
import type { Goal, ThreadTask } from "./domain";
import { errorMessage } from "./errors";
import { FindOrCreatePicker } from "./FindOrCreatePicker";
import { formatPeriod, GOAL_HORIZON_LABELS, goalOptionsForTask, periodFor } from "./goals";

type Choice = { goal: Goal | null; label: string };

/**
 * Links one task to a goal from the keyboard: type to find one of the
 * account's active goals, or a new name to create it as a goal for the
 * current quarter. "No goal" unlinks.
 */
export function GoalLinkPicker({
  task,
  goals,
  now = new Date(),
  onLink,
  onCreateAndLink,
  onClose,
}: {
  task: ThreadTask;
  goals: readonly Goal[];
  now?: Date;
  onLink(goalId: string | null): Promise<void>;
  onCreateAndLink(title: string, period: string): Promise<void>;
  onClose(): void;
}) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const choices: Choice[] = [
    { goal: null, label: "No goal" },
    ...goalOptionsForTask(goals, task).map((goal) => ({ goal, label: goal.title })),
  ];
  const quarter = periodFor("quarter", now);

  const run = async (work: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await work();
      onClose();
    } catch (reason) {
      setError(errorMessage(reason));
      setBusy(false);
    }
  };

  return (
    <Modal title={`Link “${task.title}” to a goal`} className="goal-link-modal" onClose={onClose}>
      <FindOrCreatePicker
        items={choices}
        getSearchText={(choice) => choice.label}
        placeholder="Find a goal, or name a new one"
        ariaLabel="Find or create a goal"
        listId="goal-link-options"
        listLabel="Goals"
        emptyMessage="No goals yet. Type a name to create one."
        createLabel={(title) => <>Create {formatPeriod(quarter)} goal “{title}”</>}
        onSelect={(choice) => void run(() => onLink(choice.goal?.id ?? null))}
        onCreate={(title) => void run(() => onCreateAndLink(title, quarter))}
        renderItem={(choice, option) => {
          const current = (choice.goal?.id ?? null) === (task.goalId ?? null);
          return <div
            key={choice.goal?.id ?? "none"}
            id={option.id}
            role="option"
            aria-label={current ? `${choice.label}, current` : choice.label}
            aria-selected={option.active}
            className={option.active ? "highlighted" : undefined}
            onMouseEnter={option.onMouseEnter}
            onClick={option.onClick}
          >
            <span className="label-option-name">
              {current ? <Check size={14} /> : <span className="label-option-check-spacer" />}
              {choice.label}
            </span>
            {choice.goal ? <span className="goal-link-period">{GOAL_HORIZON_LABELS[choice.goal.horizon]} · {formatPeriod(choice.goal.period)}</span> : null}
          </div>;
        }}
      />
      {error ? <p className="form-error goal-link-error" role="alert">{error}</p> : null}
    </Modal>
  );
}
