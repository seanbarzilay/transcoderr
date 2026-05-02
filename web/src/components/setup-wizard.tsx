import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import SourceStep from "./setup-wizard-steps/source";
import NotifierStep from "./setup-wizard-steps/notifier";
import PluginsStep from "./setup-wizard-steps/plugins";

type Step = "welcome" | "source" | "notifier" | "plugins" | "flow" | "done";

const STEP_ORDER: Step[] = ["welcome", "source", "notifier", "plugins", "flow", "done"];

const STEP_LABELS: Record<Step, string> = {
  welcome: "Welcome",
  source: "Source",
  notifier: "Notifier",
  plugins: "Plugins",
  flow: "First flow",
  done: "Done",
};

/// Auto-launches when the operator's instance has no sources AND the
/// `wizard.completed` settings key isn't set. Both Skip wizard and
/// Finish PATCH `wizard.completed = "true"` so the wizard never
/// reappears for that operator.
export default function SetupWizard() {
  const qc = useQueryClient();
  const sources = useQuery({ queryKey: ["sources"], queryFn: api.sources.list });
  const settings = useQuery({ queryKey: ["settings"], queryFn: api.settings.get });

  const [step, setStep] = useState<Step>("welcome");
  // Locally suppressed for this session — closing the modal hides it
  // immediately while the PATCH flies; we don't wait on the server
  // round trip to dismiss.
  const [dismissed, setDismissed] = useState(false);

  const markDone = useMutation({
    mutationFn: () => api.settings.patch({ "wizard.completed": "true" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["settings"] }),
  });

  if (sources.isLoading || settings.isLoading) return null;
  if ((sources.data ?? []).length > 0) return null;
  if (settings.data?.["wizard.completed"] === "true") return null;
  if (dismissed) return null;

  const finish = () => {
    setDismissed(true);
    markDone.mutate();
  };
  const next = (s: Step) => setStep(s);
  const back = () => {
    const i = STEP_ORDER.indexOf(step);
    if (i > 0) setStep(STEP_ORDER[i - 1]);
  };

  return (
    <div className="modal-backdrop">
      <div className="modal wizard-modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h3>Set up transcoderr</h3>
          <button
            className="btn-text"
            onClick={finish}
            title="Skip the rest of setup; the wizard won't show again."
          >
            Skip wizard
          </button>
        </div>

        <div className="wizard-body">
          <ul className="wizard-rail">
            {STEP_ORDER.filter(s => s !== "welcome" && s !== "done").map((s, i) => (
              <li
                key={s}
                className={"wizard-rail-item" + (step === s ? " is-active" : "")}
              >
                <span className="wizard-rail-num">{i + 1}</span>
                <span>{STEP_LABELS[s]}</span>
              </li>
            ))}
          </ul>

          <div className="wizard-pane">
            {step === "welcome" && <Welcome onStart={() => next("source")} />}
            {step === "source" && (
              <SourceStep
                onCreated={() => next("notifier")}
                onSkip={() => next("notifier")}
              />
            )}
            {step === "notifier" && (
              <NotifierStep
                onCreated={() => next("plugins")}
                onSkip={() => next("plugins")}
              />
            )}
            {step === "plugins" && (
              <PluginsStep
                onContinue={() => next("done")}
                onSkip={() => next("done")}
              />
            )}
            {step === "done" && <Done onFinish={finish} />}
          </div>
        </div>

        <div className="modal-footer wizard-footer">
          {step !== "welcome" && step !== "done" && (
            <button className="btn-ghost" onClick={back}>Back</button>
          )}
        </div>
      </div>
    </div>
  );
}

function Welcome({ onStart }: { onStart: () => void }) {
  return (
    <div className="wizard-step">
      <h4>Welcome to transcoderr</h4>
      <p className="muted">
        We'll walk through four quick steps to get you running:
      </p>
      <ol className="wizard-welcome-list">
        <li>Connect a media source (Radarr, Sonarr, Lidarr, or a custom webhook)</li>
        <li>Add a notifier so flows can ping you on success or failure (optional)</li>
        <li>See where to install plugins for extra step kinds (optional)</li>
        <li>Create your first flow from a starter template</li>
      </ol>
      <p className="muted" style={{ marginTop: 12 }}>
        Each step is skippable and the wizard won't reappear after you finish.
      </p>
      <div style={{ display: "flex", gap: 8, marginTop: 16 }}>
        <button onClick={onStart}>Start</button>
      </div>
    </div>
  );
}

function Done({ onFinish }: { onFinish: () => void }) {
  return (
    <div className="wizard-step">
      <h4>You're set up</h4>
      <p className="muted">
        Future *arr pushes will trigger your flow automatically. You can
        manage everything from the sidebar — Sources, Notifiers, Plugins,
        and Flows.
      </p>
      <div style={{ display: "flex", gap: 8, marginTop: 16 }}>
        <button onClick={onFinish}>Finish</button>
      </div>
    </div>
  );
}
