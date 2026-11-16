import { LoaderCircle } from "lucide-react";
import { ICON_SIZE } from "./iconSizes";

export function PullToRefreshStatus({ refreshing, error }: { refreshing: boolean; error: string | null }) {
  return <>
    {refreshing ? <p className="pull-refresh-status" role="status"><LoaderCircle size={ICON_SIZE.sm} className="spin" aria-hidden="true" />Refreshing…</p> : null}
    {error ? <p className="pull-refresh-status form-error" role="alert">Refresh failed: {error}</p> : null}
  </>;
}
