import { LayoutDashboard } from "lucide-react";
import { HoverTooltip } from "./AppChrome";
import { ContextSectionHeader } from "./ContextSections";
import { ICON_SIZE } from "./iconSizes";

/**
 * What's next once the inbox is empty: a way to Today, which holds the
 * agenda, tasks due, and people to reconnect with, rather than a second copy
 * of them beside the empty inbox.
 */
export function InboxZeroSections({ onOpenToday }: { onOpenToday(): void }) {
  return <section className="context-section inbox-zero-next" aria-label="What's next">
    <ContextSectionHeader title="What's next" icon={<LayoutDashboard size={ICON_SIZE.xs} />} />
    <p className="context-status">Your agenda, tasks due, and people to reconnect with are in Today.</p>
    <HoverTooltip label="Today" shortcut="0">
      <button type="button" className="btn btn-sm" onClick={onOpenToday}>Open Today</button>
    </HoverTooltip>
  </section>;
}
