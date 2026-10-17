import { Component, type ErrorInfo, type ReactNode } from "react";
import { CircleAlert } from "lucide-react";
import { reportCaughtError } from "./crashReporting";
import { ICON_SIZE } from "./iconSizes";

/**
 * Where the fallback sits:
 * - `pane` fills the pane it was rendered inside (the inbox, the reader).
 * - `side` stands in for a whole side pane as the app grid's item.
 * - `floating` is a card over the window, for areas such as Settings that
 *   cover the window themselves.
 * - `app` replaces the whole window and offers a reload.
 */
export type ErrorBoundaryVariant = "pane" | "side" | "floating" | "app";

type ErrorBoundaryProps = {
  /** Names the area in the message, e.g. "this conversation" or "Settings". */
  area: string;
  children: ReactNode;
  variant?: ErrorBoundaryVariant;
  /**
   * A failed area renders again when this changes, so choosing another
   * thread or mailbox recovers without a button press.
   */
  resetKey?: unknown;
  /** Adds a Close button for areas the person can dismiss. */
  onDismiss?: () => void;
};

type ErrorBoundaryState = { failed: boolean };

/**
 * Keeps one area's render failure from blanking the window. React unmounts
 * the whole tree when a render error is not caught, so each main area gets
 * its own boundary and the rest of the app keeps working. A healthy boundary
 * renders its children unchanged and adds no element of its own.
 */
export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { failed: false };

  static getDerivedStateFromError(): ErrorBoundaryState {
    return { failed: true };
  }

  componentDidCatch(error: unknown, info: ErrorInfo): void {
    reportCaughtError(`Showing ${this.props.area} failed`, error, info.componentStack);
  }

  componentDidUpdate(previous: ErrorBoundaryProps): void {
    if (this.state.failed && !Object.is(previous.resetKey, this.props.resetKey)) this.setState({ failed: false });
  }

  render(): ReactNode {
    if (!this.state.failed) return this.props.children;
    const { area, variant = "pane", onDismiss } = this.props;
    const reloads = variant === "app";
    return (
      <div className={`pane-error ${variant}`} role="alert">
        <CircleAlert size={ICON_SIZE.display} />
        <p>Something went wrong showing {area}.</p>
        <div className="pane-error-actions">
          <button
            type="button"
            className="btn"
            onClick={() => (reloads ? window.location.reload() : this.setState({ failed: false }))}
          >
            {reloads ? "Reload ThreeStrands" : "Try Again"}
          </button>
          {onDismiss ? (
            <button type="button" className="btn btn-ghost" onClick={onDismiss}>
              Close
            </button>
          ) : null}
        </div>
      </div>
    );
  }
}
