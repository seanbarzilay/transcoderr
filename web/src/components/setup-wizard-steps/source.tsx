import AddSourceForm from "../forms/add-source";

interface Props {
  onCreated: () => void;
  onSkip: () => void;
}

export default function SourceStep({ onCreated, onSkip }: Props) {
  return (
    <div className="wizard-step">
      <h4>Connect a source</h4>
      <p className="muted">
        Pick the tool you want transcoderr to listen to. Radarr / Sonarr /
        Lidarr auto-provision the webhook for you (just paste a base URL +
        API key). Choose <code>webhook</code> for anything else — you'll
        wire it from your tool's settings page.
      </p>
      <AddSourceForm onCreated={() => onCreated()} />
      <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
        <button className="btn-ghost" onClick={onSkip}>Skip this step</button>
      </div>
    </div>
  );
}
