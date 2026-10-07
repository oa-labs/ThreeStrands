/**
 * Icon sizes by role. Pick the role that matches the icon's container, never a
 * raw number; see DESIGN.md, Icons.
 */
export const ICON_SIZE = {
  /** Inline with meta or secondary text, and chip remove buttons. */
  xs: 12,
  /** Small buttons (.btn-sm, .btn-icon-sm) and body-text indicators. */
  sm: 14,
  /** Standard buttons, segments, and fields. */
  md: 16,
  /** Icon buttons, the app rail, and pane-level indicators. */
  lg: 18,
  /** Empty-state illustrations. */
  display: 28,
} as const;
