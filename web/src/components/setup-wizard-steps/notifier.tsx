import AddNotifierForm from "../forms/add-notifier";

interface Props {
  onCreated: () => void;
  onSkip: () => void;
}

export default function NotifierStep({ onCreated, onSkip }: Props) {
  return (
    <div className="wizard-step">
      <h4>Add a notifier (optional)</h4>
      <p className="muted">
        Notifiers let flows ping you on start, success, or failure via
        Discord, Telegram, ntfy, a custom webhook, or Jellyfin (which
        triggers a per-file rescan). Skip if you don't need notifications
        — you can add them later under the Notifiers tab.
      </p>
      <AddNotifierForm onCreated={() => onCreated()} />
      <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
        <button className="btn-ghost" onClick={onSkip}>Skip this step</button>
      </div>
    </div>
  );
}
