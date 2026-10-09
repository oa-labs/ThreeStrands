import { useEffect, useState } from "react";
import { getRetentionDays, setRetentionDays, RETENTION_OPTIONS } from "./retentionSettings";

export function PrivacySettings({
  loadRemoteImages,
  onLoadRemoteImagesChange,
}: {
  loadRemoteImages: boolean;
  onLoadRemoteImagesChange(value: boolean): void;
}) {
  const [retentionDays, setRetentionDaysState] = useState<number | null>(null);

  useEffect(() => {
    void getRetentionDays().then(setRetentionDaysState);
  }, []);

  return (
    <section className="settings-section wide-label-settings" aria-label="Privacy">
      <h3>Local Storage</h3>
      <label className="settings-field settings-field-row">
        <span>Keep Mail on This Device For</span>
        <select
          value={retentionDays === null ? "forever" : String(retentionDays)}
          onChange={(event) => {
            const next = event.target.value === "forever" ? null : Number(event.target.value);
            setRetentionDaysState(next);
            void setRetentionDays(next);
          }}
        >
          {RETENTION_OPTIONS.map((option) => (
            <option key={option.label} value={option.value === null ? "forever" : String(option.value)}>
              {option.label}
            </option>
          ))}
        </select>
      </label>
      <span className="settings-hint settings-field-detail">
        Mail older than this is removed from ThreeStrands's local cache to keep the
        database from growing without bound. It stays on the server.
      </span>

      <h3>Message Images</h3>
      <label className="settings-switch">
        <span>Load Remote Images Automatically</span>
        <input
          type="checkbox"
          checked={loadRemoteImages}
          onChange={(event) => onLoadRemoteImagesChange(event.target.checked)}
        />
      </label>
      <span className="settings-hint">
        When disabled, images stay blocked until you choose Load images in a message.
      </span>

    </section>
  );
}
