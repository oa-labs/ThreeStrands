import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import type { Account } from "./domain";

/** How long after an account change a freshly mounted chip still plays the highlight pulse. */
const RECENT_CHANGE_MS = 700;

type AccountScope = { accounts: Account[]; changedAt: number };

const AccountScopeContext = createContext<AccountScope>({ accounts: [], changedAt: 0 });

/** Shares the account list and the moment the active account last changed with every scope chip. */
export function AccountScopeProvider({ accounts, activeAccountId, children }: {
  accounts: Account[];
  activeAccountId: string | null;
  children: ReactNode;
}) {
  const [changedAt, setChangedAt] = useState(0);
  const previous = useRef(activeAccountId);
  useEffect(() => {
    if (previous.current === activeAccountId) return;
    previous.current = activeAccountId;
    setChangedAt(Date.now());
  }, [activeAccountId]);
  return <AccountScopeContext.Provider value={{ accounts, changedAt }}>{children}</AccountScopeContext.Provider>;
}

export function accountScopeName(account: Account): string {
  return account.nickname?.trim() || account.displayName || account.email;
}

/** A colour dot and name showing which account an account-scoped view is filtered to. */
export function AccountScopeChip({ accountId }: { accountId: string | null }) {
  const { accounts, changedAt } = useContext(AccountScopeContext);
  const account = accountId ? accounts.find((candidate) => candidate.email === accountId) : undefined;
  const label = accountId === null ? "All accounts" : account ? accountScopeName(account) : accountId;
  const recent = () => changedAt > 0 && Date.now() - changedAt < RECENT_CHANGE_MS;
  const [pulse, setPulse] = useState(() => (recent() ? changedAt : 0));
  useEffect(() => {
    if (recent()) setPulse(changedAt);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [changedAt]);
  return (
    <span
      key={pulse}
      className={`account-scope-chip${pulse ? " pulse" : ""}`}
      data-all={accountId === null ? "true" : undefined}
      style={account?.color ? { "--scope-color": account.color } as React.CSSProperties : undefined}
      title={account && account.email !== label ? account.email : undefined}
    >
      <span className="account-scope-dot" aria-hidden="true" />
      <span className="account-scope-name">{label}</span>
    </span>
  );
}
