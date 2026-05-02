interface Props {
  onContinue: () => void;
  onSkip: () => void;
}

export default function PluginsStep({ onContinue, onSkip }: Props) {
  return (
    <div className="wizard-step">
      <h4>Plugins (optional)</h4>
      <p className="muted">
        Plugins add custom step kinds to flows — e.g. <code>size-report</code>
        for compression stats, <code>whisper.transcribe</code> for
        auto-generated subtitles. Browse the official catalog and install
        with one click after you finish the wizard.
      </p>
      <p className="muted">
        You can come back to this from the <strong>Plugins</strong> tab in
        the sidebar at any time.
      </p>
      <div style={{ display: "flex", gap: 8, marginTop: 12 }}>
        <button onClick={onContinue}>Continue</button>
        <button className="btn-ghost" onClick={onSkip}>Skip this step</button>
      </div>
    </div>
  );
}
