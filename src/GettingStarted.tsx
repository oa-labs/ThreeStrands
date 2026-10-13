import { openUrl } from "@tauri-apps/plugin-opener";
import { Circle, CircleCheck } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Modal } from "./AppChrome";
import {
  completeSteps,
  docsUrl,
  GETTING_STARTED_STEPS,
  readGettingStarted,
  remainingSteps,
  satisfiedFactSteps,
  saveGettingStarted,
  showGettingStarted,
  type GettingStartedFacts,
  type GettingStartedState,
  type GettingStartedStepId,
} from "./gettingStartedSteps";
import { ICON_SIZE } from "./iconSizes";

export function useGettingStarted({ mailConnected, calendarConnected, aiReady }: GettingStartedFacts) {
  const [state, setState] = useState<GettingStartedState>(readGettingStarted);
  const update = useCallback((change: (current: GettingStartedState) => GettingStartedState) => {
    setState((current) => {
      const next = change(current);
      if (next !== current) saveGettingStarted(next);
      return next;
    });
  }, []);
  useEffect(() => {
    update((current) => completeSteps(current, satisfiedFactSteps({ mailConnected, calendarConnected, aiReady })));
  }, [mailConnected, calendarConnected, aiReady, update]);
  const complete = useCallback((id: GettingStartedStepId) => update((current) => completeSteps(current, [id])), [update]);
  const dismiss = useCallback(() => update((current) => (current.dismissed ? current : { ...current, dismissed: true })), [update]);
  return { state, visible: showGettingStarted(state), remaining: remainingSteps(state).length, complete, dismiss };
}

/** Opens a docs page in the system browser; the webview never navigates away from the app. */
export function openDocs(slug: string, section?: string) {
  void openUrl(docsUrl(slug, section)).catch(() => {});
}

export function GettingStartedDialog({
  state,
  onAction,
  onDismiss,
  onClose,
}: {
  state: GettingStartedState;
  onAction(id: GettingStartedStepId): void;
  onDismiss(): void;
  onClose(): void;
}) {
  const doneCount = GETTING_STARTED_STEPS.filter((step) => state.completed.includes(step.id)).length;
  return (
    <Modal title="Getting Started" className="getting-started-modal" onClose={onClose}>
      <p className="getting-started-intro">A few steps to make ThreeStrands yours. Each one checks itself off when you do it.</p>
      <div className="getting-started-progress">
        <progress value={doneCount} max={GETTING_STARTED_STEPS.length} aria-label="Getting Started progress" />
        <span>{doneCount} of {GETTING_STARTED_STEPS.length} done</span>
      </div>
      <ol className="getting-started-steps">
        {GETTING_STARTED_STEPS.map((step) => {
          const done = state.completed.includes(step.id);
          return (
            <li key={step.id} className={done ? "done" : undefined} aria-label={`${step.title}${done ? ", done" : ""}`}>
              {done ? <CircleCheck size={ICON_SIZE.md} aria-hidden="true" /> : <Circle size={ICON_SIZE.md} aria-hidden="true" />}
              <div className="getting-started-step">
                <strong>{step.title}</strong>
                {step.optional ? <span className="getting-started-optional">Optional</span> : null}
                <p>{step.description}</p>
                <div className="getting-started-step-actions">
                  {done ? null : <button type="button" className="btn btn-sm" onClick={() => onAction(step.id)}>{step.actionLabel}</button>}
                  <button type="button" className="btn-link" onClick={() => openDocs(step.docSlug, step.docSection)}>
                    Learn More<span className="sr-only">: {step.title}</span>
                  </button>
                </div>
              </div>
            </li>
          );
        })}
      </ol>
      <div className="modal-form-actions getting-started-actions">
        <button type="button" className="btn-link" onClick={() => openDocs("index")}>Read the Docs</button>
        <button type="button" className="btn btn-ghost" onClick={onDismiss}>Hide Getting Started</button>
      </div>
    </Modal>
  );
}
