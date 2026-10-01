import { useState } from "react";

export type FrontierCandidate = {
  operationId: string;
  deviceId: string;
  value: unknown;
};

export type FrontierConflict = {
  entityType: string;
  entityId: string;
  field: string;
  candidates: FrontierCandidate[];
};

/**
 * Resolves a field-level conflict from the replicated-sync operation graph:
 * an N-way choice among every operation still in the field's frontier,
 * one per concurrent writer that actually raced. Resolving picks one candidate's value; the caller emits
 * the resulting operation with `parents` set to the entire current
 * frontier, per the sync protocol's conflict-resolution contract.
 *
 * Not yet wired to a live backend command: no transport can produce a
 * multi-device conflict until a later phase. This component is ready for
 * that phase to hand real conflicts to.
 */
export function FrontierConflictEditor({
  conflict,
  disabled,
  onResolve,
}: {
  conflict: FrontierConflict;
  disabled: boolean;
  onResolve(chosen: FrontierCandidate): void;
}) {
  const [selectedOperationId, setSelectedOperationId] = useState(conflict.candidates[0]?.operationId);
  const resolve = () => {
    const chosen = conflict.candidates.find((candidate) => candidate.operationId === selectedOperationId);
    if (chosen) onResolve(chosen);
  };
  return (
    <div className="notice accounts-config-notice frontier-conflict-editor">
      <strong>{conflict.entityType.replaceAll("_", " ")} conflict: {conflict.field}</strong>
      <fieldset className="settings-field">
        <legend>{conflict.candidates.length} concurrent {conflict.candidates.length === 1 ? "value" : "values"}</legend>
        {conflict.candidates.map((candidate) => (
          <label key={candidate.operationId}>
            <input
              type="radio"
              name={`${conflict.entityId}-${conflict.field}`}
              checked={selectedOperationId === candidate.operationId}
              onChange={() => setSelectedOperationId(candidate.operationId)}
            />
            <span>Device {candidate.deviceId.slice(0, 8)}: {JSON.stringify(candidate.value)}</span>
          </label>
        ))}
      </fieldset>
      <button type="button" className="primary-action" disabled={disabled || !selectedOperationId} onClick={resolve}>
        Resolve conflict
      </button>
    </div>
  );
}
