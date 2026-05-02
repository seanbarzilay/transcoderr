# Setup Wizard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** First-time operators see a guided modal that walks them through setting up a source, an optional notifier, plugins, and their first flow — making the showcase site's "the web UI walks you through" claim true.

**Architecture:** A new React modal `<SetupWizard />` mounted at the layout root in `web/src/App.tsx`. Auto-launches when `api.sources.list()` returns empty AND the settings KV `wizard.completed` is not `"true"`. Six views: Welcome / Source / Notifier (skippable) / Plugins (informational) / Flow / Done. Skip wizard or Finish PATCH `wizard.completed = "true"` to the existing `/api/settings`. No backend changes. The Sources and Notifiers pages' add-form bodies are first factored out into reusable `<AddSourceForm />` / `<AddNotifierForm />` components so the wizard reuses, not forks, them.

**Tech Stack:** React 18, TypeScript, TanStack Query v5, react-router-dom, vite. Existing `.modal-*` CSS classes from `web/src/index.css` (added by `install-log-modal`).

**Branch:** all tasks land on a fresh `feat/setup-wizard` branch off `main`. Implementer creates the branch before Task 1.

**Tests:** the web tree has no test infrastructure today (no vitest, no test files). Setting that up is out of scope for this PR. Verification is via build smoke test (`npm --prefix web run build`) plus the manual end-to-end checks at the end of each task.

---

## File Structure

**New files:**
- `web/src/components/forms/add-source.tsx` — extracted from `pages/sources.tsx`. Renders the kind picker + name + per-kind fields + Add button. Props: `{ onCreated: (id: number) => void }`.
- `web/src/components/forms/add-notifier.tsx` — extracted from `pages/notifiers.tsx`. Same shape: `{ onCreated: (id: number) => void }`.
- `web/src/components/setup-wizard.tsx` — modal shell, step state, gating, persistence. Renders `<Welcome />`, `<Done />`, plus the four step views as they're built.
- `web/src/components/setup-wizard-steps/source.tsx` — wraps `<AddSourceForm />` with wizard wiring.
- `web/src/components/setup-wizard-steps/notifier.tsx` — wraps `<AddNotifierForm />`.
- `web/src/components/setup-wizard-steps/plugins.tsx` — informational pane.
- `web/src/components/setup-wizard-steps/flow.tsx` — template-loaded textarea + Save.
- `web/src/templates/hevc-normalize.yaml` — verbatim copy of `docs/flows/hevc-normalize.yaml`, imported via `?raw`.

**Modified files:**
- `web/src/pages/sources.tsx` — uses `<AddSourceForm />` instead of inlining.
- `web/src/pages/notifiers.tsx` — uses `<AddNotifierForm />` instead of inlining.
- `web/src/App.tsx` — mounts `<SetupWizard />` at the layout root.
- `web/src/index.css` — small additions for the wizard's two-column inner layout (`.wizard-rail`, `.wizard-pane`, step-list styles).

---

## Task 1: Extract `<AddSourceForm />`

Pure refactor. Move the add-source form body out of `sources.tsx` into a reusable component. No UX change.

**Files:**
- Create: `web/src/components/forms/add-source.tsx`
- Modify: `web/src/pages/sources.tsx`

- [ ] **Step 1: Create `web/src/components/forms/add-source.tsx`**

```tsx
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../../api/client";

const AUTO_KINDS = ["radarr", "sonarr", "lidarr"] as const;

function isAutoKind(kind: string): boolean {
  return (AUTO_KINDS as readonly string[]).includes(kind);
}

function capitalize(s: string): string {
  return s.length ? s[0].toUpperCase() + s.slice(1) : s;
}

interface Props {
  /// Called with the new source's id after a successful create. Use this to
  /// advance the setup wizard, navigate somewhere, etc.
  onCreated?: (id: number) => void;
}

export default function AddSourceForm({ onCreated }: Props) {
  const qc = useQueryClient();
  const [kind, setKind] = useState<string>("radarr");
  const [name, setName] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [secretToken, setSecretToken] = useState("");
  const [config, setConfig] = useState("{}");
  const [formError, setFormError] = useState<string | null>(null);

  const isAutoProvision = isAutoKind(kind);

  const create = useMutation({
    mutationFn: (body: any) => api.sources.create(body),
    onSuccess: (resp) => {
      setName("");
      setBaseUrl("");
      setApiKey("");
      setSecretToken("");
      setConfig("{}");
      setFormError(null);
      qc.invalidateQueries({ queryKey: ["sources"] });
      onCreated?.(resp.id);
    },
    onError: (e: any) => {
      setFormError(e?.message ?? String(e));
    },
  });

  const submit = () => {
    setFormError(null);
    if (isAutoProvision) {
      create.mutate({
        kind,
        name,
        config: { base_url: baseUrl, api_key: apiKey },
        secret_token: "",
      });
    } else {
      let parsed: any;
      try {
        parsed = JSON.parse(config || "{}");
      } catch (e: any) {
        setFormError(`Invalid config JSON: ${e?.message ?? e}`);
        return;
      }
      create.mutate({
        kind,
        name,
        config: parsed,
        secret_token: secretToken,
      });
    }
  };

  const canSubmit = (() => {
    if (!name.trim()) return false;
    if (isAutoProvision) return baseUrl.trim() !== "" && apiKey.trim() !== "";
    return secretToken.trim() !== "";
  })();

  return (
    <div className="surface" style={{ padding: 16, marginBottom: 16 }}>
      <div className="label" style={{ marginBottom: 8 }}>
        Add source
      </div>
      <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
        <select value={kind} onChange={(e) => setKind(e.target.value)}>
          <option value="radarr">radarr</option>
          <option value="sonarr">sonarr</option>
          <option value="lidarr">lidarr</option>
          <option value="webhook">webhook</option>
        </select>
        <input
          placeholder="name"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        {isAutoProvision ? (
          <>
            <input
              placeholder={`base url (e.g. http://${kind}:${
                kind === "radarr" ? "7878" : kind === "sonarr" ? "8989" : "8686"
              })`}
              value={baseUrl}
              onChange={(e) => setBaseUrl(e.target.value)}
              style={{ flex: 1, minWidth: 240 }}
            />
            <input
              type="password"
              placeholder="api key"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
            />
          </>
        ) : (
          <>
            <input
              placeholder="secret token"
              value={secretToken}
              onChange={(e) => setSecretToken(e.target.value)}
            />
            <input
              placeholder="config json (e.g. {})"
              value={config}
              onChange={(e) => setConfig(e.target.value)}
              style={{ flex: 1, minWidth: 240 }}
            />
          </>
        )}
        <button onClick={submit} disabled={!canSubmit || create.isPending}>
          Add
        </button>
      </div>
      {isAutoProvision ? (
        <p className="hint">
          Transcoderr will create the webhook in {capitalize(kind)} for you.
          The connection token is generated automatically.
        </p>
      ) : (
        <p className="hint">
          Add a webhook in your tool's settings pointing at{" "}
          <code>{window.location.origin}/webhook/{name || "&lt;name&gt;"}</code> with the
          secret token above as the password.
        </p>
      )}
      {formError && (
        <p className="hint" style={{ color: "var(--bad)" }}>
          {formError}
        </p>
      )}
    </div>
  );
}
```

- [ ] **Step 2: Replace the form body in `web/src/pages/sources.tsx`**

Replace the entire content of `sources.tsx` (the page component, not the helpers) so it imports `<AddSourceForm />` and keeps only the table:

```tsx
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import type { Source } from "../types";
import AddSourceForm from "../components/forms/add-source";

const AUTO_KINDS = ["radarr", "sonarr", "lidarr"] as const;

function isAutoKind(kind: string): boolean {
  return (AUTO_KINDS as readonly string[]).includes(kind);
}

function isAutoSource(src: Source): boolean {
  return isAutoKind(src.kind) && src.config?.arr_notification_id != null;
}

export default function Sources() {
  const qc = useQueryClient();
  const sources = useQuery({ queryKey: ["sources"], queryFn: api.sources.list });

  const del = useMutation({
    mutationFn: (id: number) => api.sources.delete(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["sources"] }),
  });

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <div className="crumb">Configure</div>
          <h2>Sources</h2>
        </div>
      </div>

      <AddSourceForm />

      <div className="surface">
        <table>
          <thead>
            <tr>
              <th style={{ width: 100 }}>Kind</th>
              <th style={{ width: 90 }}>Mode</th>
              <th style={{ width: 180 }}>Name</th>
              <th>Webhook URL</th>
              <th style={{ width: 110 }}></th>
            </tr>
          </thead>
          <tbody>
            {(sources.data ?? []).map((s: Source) => (
              <tr key={s.id}>
                <td>
                  <span className="label">{s.kind}</span>
                </td>
                <td>
                  <span
                    className={`badge badge-${
                      isAutoSource(s) ? "auto" : "manual"
                    }`}
                  >
                    {isAutoSource(s) ? "auto" : "manual"}
                  </span>
                </td>
                <td>{s.name}</td>
                <td className="mono dim">
                  {s.kind === "webhook"
                    ? `/webhook/${s.name}`
                    : `/webhook/${s.kind}`}
                </td>
                <td>
                  <button
                    className="btn-danger"
                    onClick={() => del.mutate(s.id)}
                  >
                    Delete
                  </button>
                </td>
              </tr>
            ))}
            {(sources.data ?? []).length === 0 && !sources.isLoading && (
              <tr>
                <td colSpan={5} className="empty">
                  No sources configured.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Verify build**

```bash
npm --prefix web run build 2>&1 | tail -10
```

Expected: build succeeds, no TS errors.

- [ ] **Step 4: Manual smoke test**

```bash
npm --prefix web run dev
```

Expected: open `http://localhost:5173/sources`, see the same Add Source form as before, add a test radarr source, see it appear in the table, delete it. UX should be identical to before this task.

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/setup-wizard" || { echo "WRONG BRANCH"; exit 1; }
git add web/src/components/forms/add-source.tsx web/src/pages/sources.tsx
git commit -m "web: extract <AddSourceForm /> from Sources page"
```

---

## Task 2: Extract `<AddNotifierForm />`

Same shape as Task 1, for notifiers. The page already uses a factored `<NotifierForm />` for the per-kind config inputs; this task extracts the *wrapping* add-form (kind picker + name + config + Add button).

**Files:**
- Create: `web/src/components/forms/add-notifier.tsx`
- Modify: `web/src/pages/notifiers.tsx`

- [ ] **Step 1: Create `web/src/components/forms/add-notifier.tsx`**

```tsx
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../../api/client";
import NotifierForm, {
  KINDS,
  emptyForm,
  toConfig,
  validate,
} from "../notifier-form";
import type { Kind, FormValue } from "../notifier-form";

interface Props {
  /// Called with the new notifier's id after a successful create.
  onCreated?: (id: number) => void;
}

export default function AddNotifierForm({ onCreated }: Props) {
  const qc = useQueryClient();
  const [kind, setKind] = useState<Kind>("discord");
  const [name, setName] = useState("");
  const [value, setValue] = useState<FormValue>(() => emptyForm("discord"));
  const [error, setError] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: () => {
      const config = toConfig(kind, value, false);
      return api.notifiers.create({ name, kind, config });
    },
    onSuccess: (resp) => {
      setName("");
      setValue(emptyForm(kind));
      setError(null);
      qc.invalidateQueries({ queryKey: ["notifiers"] });
      onCreated?.(resp.id);
    },
    onError: (e: any) => setError(e?.message ?? "create failed"),
  });

  const validationError = validate(kind, value, false);
  const disabled = create.isPending || !name.trim() || validationError !== null;

  return (
    <div className="surface" style={{ padding: 16, marginBottom: 16 }}>
      <div className="label" style={{ marginBottom: 8 }}>
        Add notifier
      </div>
      <div className="notifier-add">
        <div className="notifier-add-head">
          <select
            value={kind}
            onChange={e => {
              const k = e.target.value as Kind;
              setKind(k);
              setValue(emptyForm(k));
            }}
          >
            {KINDS.map(k => (
              <option key={k}>{k}</option>
            ))}
          </select>
          <input
            placeholder="name (referenced from flow YAML)"
            value={name}
            onChange={e => setName(e.target.value)}
            style={{ flex: 1, minWidth: 220 }}
          />
        </div>
        <NotifierForm
          kind={kind}
          value={value}
          onChange={setValue}
          isEdit={false}
        />
        <div style={{ display: "flex", gap: 8, alignItems: "center", marginTop: 8 }}>
          <button onClick={() => create.mutate()} disabled={disabled}>
            Add
          </button>
          {!create.isPending && validationError && name.trim() && (
            <span className="notifier-form-hint">{validationError}</span>
          )}
        </div>
      </div>
      {error && (
        <div style={{ color: "var(--bad)", marginTop: 8, fontSize: 12 }}>{error}</div>
      )}
    </div>
  );
}
```

- [ ] **Step 2: Update `web/src/pages/notifiers.tsx`**

Read the current file first, then replace ONLY the body of the `Notifiers` component up through the closing of the "Add notifier" surface div with the import + `<AddNotifierForm />`. The rest of the page (the existing notifiers table with inline edit) stays unchanged.

The diff: drop the `kind`, `name`, `addValue`, `addError` `useState` hooks, drop the `create` mutation, drop everything inside the `<div className="surface" style={{ padding: 16, marginBottom: 16 }}>` block that holds Add notifier. Replace that surface div with `<AddNotifierForm />`. Drop the now-unused imports (`emptyForm`, `toConfig`, `validate` from `notifier-form` — leave `NotifierForm`, `KINDS`, `fromConfig`, `Kind`, `FormValue` because the table's edit UI still uses them).

Sanity check: `notifiers.tsx` should still have the inline-edit table at the bottom; only the top "Add notifier" surface block goes.

- [ ] **Step 3: Verify build**

```bash
npm --prefix web run build 2>&1 | tail -10
```

Expected: build succeeds, no TS errors.

- [ ] **Step 4: Manual smoke test**

```bash
npm --prefix web run dev
```

Expected: at `/notifiers`, the Add notifier form looks and behaves identically to before. Add a test discord notifier, see it appear, edit it, delete it — all UX unchanged.

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/setup-wizard" || { echo "WRONG BRANCH"; exit 1; }
git add web/src/components/forms/add-notifier.tsx web/src/pages/notifiers.tsx
git commit -m "web: extract <AddNotifierForm /> from Notifiers page"
```

---

## Task 3: SetupWizard shell + Welcome + Done + mount in App

Ship the modal shell with gating + persistence wired up. Only Welcome and Done step views exist for now — clicking Start in Welcome jumps directly to Done. Subsequent tasks add the four real step views between them. This makes the gating + persist + close path testable end-to-end starting now.

**Files:**
- Create: `web/src/components/setup-wizard.tsx`
- Modify: `web/src/App.tsx`
- Modify: `web/src/index.css`

- [ ] **Step 1: Create `web/src/components/setup-wizard.tsx`**

```tsx
import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";

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
            {step === "welcome" && <Welcome onStart={() => next("done")} />}
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
```

- [ ] **Step 2: Mount `<SetupWizard />` in `web/src/App.tsx`**

Read the current `App.tsx`, then add the import at the top and the mount inside the `app-shell` div, AFTER the closing `</main>` so the modal overlays the main content.

```tsx
import SetupWizard from "./components/setup-wizard";
```

Inside the return, after `</main>` and before `</div>` of the app-shell:

```tsx
<SetupWizard />
```

- [ ] **Step 3: Add wizard styles to `web/src/index.css`**

Append at the bottom of `web/src/index.css`:

```css
/* ---- setup wizard --------------------------------------------------------- */

.wizard-modal {
  width: min(900px, 100%);
  max-height: min(85vh, 760px);
}
.wizard-body {
  display: grid;
  grid-template-columns: 200px 1fr;
  flex: 1;
  min-height: 0;
}
.wizard-rail {
  list-style: none;
  margin: 0;
  padding: var(--space-4) var(--space-3);
  border-right: 1px solid var(--border);
  background: var(--surface-2);
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.wizard-rail-item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 10px;
  border-radius: var(--r-2);
  font-size: 13px;
  color: var(--text-dim);
}
.wizard-rail-item.is-active {
  background: var(--surface-3);
  color: var(--text);
}
.wizard-rail-num {
  display: inline-flex;
  width: 22px; height: 22px;
  align-items: center;
  justify-content: center;
  border-radius: 999px;
  background: var(--bg);
  border: 1px solid var(--border);
  font-size: 11px;
  font-family: var(--font-mono);
}
.wizard-rail-item.is-active .wizard-rail-num {
  border-color: var(--accent);
  color: var(--accent);
}
.wizard-pane {
  padding: var(--space-5);
  overflow-y: auto;
}
.wizard-step h4 { margin: 0 0 8px; font-size: 16px; }
.wizard-step p { margin: 0 0 12px; font-size: 13px; }
.wizard-welcome-list { padding-left: 18px; margin: 8px 0; font-size: 13px; }
.wizard-welcome-list li { padding: 3px 0; color: var(--text-dim); }
.wizard-footer { display: flex; justify-content: flex-start; gap: 8px; }

@media (max-width: 640px) {
  .wizard-body { grid-template-columns: 1fr; }
  .wizard-rail { display: none; }
}
```

- [ ] **Step 4: Verify build + smoke test**

```bash
npm --prefix web run build 2>&1 | tail -5
```

Expected: clean build.

```bash
npm --prefix web run dev
```

Expected behavior on a fresh data dir (no sources, no `wizard.completed` setting):
- Open `http://localhost:5173/dashboard`. Wizard modal appears over the dashboard.
- Click Start. See the "You're set up" Done view.
- Click Finish. Modal disappears.
- Refresh browser. Modal does NOT reappear.

On an instance that already has a source: modal should NOT appear at all.

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/setup-wizard" || { echo "WRONG BRANCH"; exit 1; }
git add web/src/components/setup-wizard.tsx web/src/App.tsx web/src/index.css
git commit -m "web: setup-wizard shell + Welcome/Done + mount + persistence"
```

---

## Task 4: Source step

The wizard's first real step. Wraps `<AddSourceForm />` and advances to the next step on successful create.

**Files:**
- Create: `web/src/components/setup-wizard-steps/source.tsx`
- Modify: `web/src/components/setup-wizard.tsx`

- [ ] **Step 1: Create `web/src/components/setup-wizard-steps/source.tsx`**

```tsx
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
```

- [ ] **Step 2: Wire it into `setup-wizard.tsx`**

In `web/src/components/setup-wizard.tsx`, add the import:

```tsx
import SourceStep from "./setup-wizard-steps/source";
```

Change the Welcome's `onStart` from `() => next("done")` to `() => next("source")`. Add a render branch in the wizard-pane:

```tsx
{step === "source" && (
  <SourceStep
    onCreated={() => next("done")}
    onSkip={() => next("done")}
  />
)}
```

Keep `next("done")` for now — Tasks 5/6/7 will replace these with `next("notifier")` etc. as the next steps land.

- [ ] **Step 3: Build + smoke test**

```bash
npm --prefix web run build 2>&1 | tail -5
```

Smoke test:
- Fresh data dir → wizard appears → Welcome → Start → Source step.
- Add a real radarr or webhook source. Wizard advances to Done.
- Or click Skip this step. Wizard advances to Done.
- Finish. Refresh browser. Wizard does not reappear.

- [ ] **Step 4: Commit**

```bash
test "$(git branch --show-current)" = "feat/setup-wizard" || { echo "WRONG BRANCH"; exit 1; }
git add web/src/components/setup-wizard-steps/source.tsx web/src/components/setup-wizard.tsx
git commit -m "web: wizard step — Source"
```

---

## Task 5: Notifier step (skippable)

**Files:**
- Create: `web/src/components/setup-wizard-steps/notifier.tsx`
- Modify: `web/src/components/setup-wizard.tsx`

- [ ] **Step 1: Create `web/src/components/setup-wizard-steps/notifier.tsx`**

```tsx
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
```

- [ ] **Step 2: Wire into `setup-wizard.tsx`**

Add the import at the top:

```tsx
import NotifierStep from "./setup-wizard-steps/notifier";
```

Update the Source step's wiring to advance to `notifier` instead of `done`:

```tsx
{step === "source" && (
  <SourceStep
    onCreated={() => next("notifier")}
    onSkip={() => next("notifier")}
  />
)}
{step === "notifier" && (
  <NotifierStep
    onCreated={() => next("done")}
    onSkip={() => next("done")}
  />
)}
```

- [ ] **Step 3: Build + smoke test**

```bash
npm --prefix web run build 2>&1 | tail -5
```

Smoke test: Welcome → Source → Notifier (add one or skip) → Done. Both skip and create paths advance.

- [ ] **Step 4: Commit**

```bash
test "$(git branch --show-current)" = "feat/setup-wizard" || { echo "WRONG BRANCH"; exit 1; }
git add web/src/components/setup-wizard-steps/notifier.tsx web/src/components/setup-wizard.tsx
git commit -m "web: wizard step — Notifier"
```

---

## Task 6: Plugins step (informational)

**Files:**
- Create: `web/src/components/setup-wizard-steps/plugins.tsx`
- Modify: `web/src/components/setup-wizard.tsx`

- [ ] **Step 1: Create `web/src/components/setup-wizard-steps/plugins.tsx`**

```tsx
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
```

- [ ] **Step 2: Wire into `setup-wizard.tsx`**

Add the import at the top:

```tsx
import PluginsStep from "./setup-wizard-steps/plugins";
```

Update Notifier step's wiring to advance to `plugins`:

```tsx
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
```

- [ ] **Step 3: Build + smoke test**

```bash
npm --prefix web run build 2>&1 | tail -5
```

Smoke test: Welcome → Source → Notifier → Plugins → Done. Continue and Skip both advance.

- [ ] **Step 4: Commit**

```bash
test "$(git branch --show-current)" = "feat/setup-wizard" || { echo "WRONG BRANCH"; exit 1; }
git add web/src/components/setup-wizard-steps/plugins.tsx web/src/components/setup-wizard.tsx
git commit -m "web: wizard step — Plugins (informational)"
```

---

## Task 7: Flow step + hevc-normalize template

The last real step. Pre-fills a textarea with the `hevc-normalize` template, lets the operator edit, and creates the flow on Save.

**Files:**
- Create: `web/src/templates/hevc-normalize.yaml`
- Create: `web/src/components/setup-wizard-steps/flow.tsx`
- Modify: `web/src/components/setup-wizard.tsx`

- [ ] **Step 1: Create `web/src/templates/hevc-normalize.yaml`**

Verbatim copy of `docs/flows/hevc-normalize.yaml`. Inline contents (use this exactly):

```yaml
name: hevc-normalize
description: |
  Re-encode anything not already hevc to x265 fast crf=19, ensure an English
  AC3 6ch audio track exists, and drop cover-art / data streams. Every
  per-stream decision is built up declaratively (`plan.*` steps) and
  materialized in a SINGLE ffmpeg pass via `plan.execute` — no chained tmp
  files, all subtitle tracks preserved.
enabled: true

triggers:
  - radarr: [downloaded, upgraded]
  - sonarr: [downloaded]

# Skip empty / unreadable files.
match:
  expr: file.size_gb > 0.001

steps:
  - id: probe
    use: probe

  - id: plan-init
    use: plan.init
  - id: plan-tolerate
    use: plan.input.tolerate_errors
  - id: plan-drop-cover-art
    use: plan.streams.drop_cover_art
  - id: plan-drop-data
    use: plan.streams.drop_data
  - id: plan-drop-unsupported-subs
    use: plan.subs.drop_unsupported

  # If the video is NOT already hevc, mark it for re-encode.
  - id: codec-gate
    if: probe.streams[0].codec_name == "hevc"
    then: []
    else:
      - id: plan-encode-video
        use: plan.video.encode
        with:
          codec: x265
          crf: 19
          preset: fast
          preserve_10bit: true
          hw:
            prefer: [nvenc, qsv, vaapi, videotoolbox]
            fallback: cpu

  - id: plan-ensure-audio
    use: plan.audio.ensure
    with:
      codec: ac3
      channels: 6
      language: eng
      dedupe: true

  # ONE ffmpeg invocation: re-encode video (or copy), copy every kept stream,
  # add the new audio track if any, write to mkv.
  - id: plan-execute
    use: plan.execute

  - id: verify
    use: verify.playable
    with: { min_duration_ratio: 0.99 }

  - id: swap
    use: output
    with: { mode: replace }
```

(Note: the live `docs/flows/hevc-normalize.yaml` includes notify steps tied to a `tg-main` channel that may not exist in a fresh install. The template above strips those — operator can add them later if they configured a notifier in the wizard's Step 5.)

- [ ] **Step 2: Confirm Vite supports `?raw` imports for `.yaml`**

```bash
grep -n "?raw" /Users/seanbarzilay/projects/private/transcoderr/web/src/**/*.tsx 2>/dev/null | head
```

Expected: at least one match (existing pages already use `?raw`). Vite supports `?raw` for any file extension; no config needed.

- [ ] **Step 3: Create `web/src/components/setup-wizard-steps/flow.tsx`**

```tsx
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../../api/client";
import hevcNormalizeYaml from "../../templates/hevc-normalize.yaml?raw";

interface Props {
  onCreated: () => void;
  onSkip: () => void;
}

export default function FlowStep({ onCreated, onSkip }: Props) {
  const qc = useQueryClient();
  const [name, setName] = useState("hevc-normalize");
  const [yaml, setYaml] = useState(hevcNormalizeYaml);
  const [error, setError] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: () => api.flows.create({ name, yaml }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["flows"] });
      onCreated();
    },
    onError: (e: any) => setError(e?.message ?? "failed to create flow"),
  });

  const disabled = create.isPending || !name.trim() || !yaml.trim();

  return (
    <div className="wizard-step">
      <h4>Create your first flow</h4>
      <p className="muted">
        Pre-loaded with <code>hevc-normalize</code>: re-encodes anything that
        isn't already HEVC to x265 (CRF 19, fast preset), ensures an English
        AC3 6ch audio track, drops cover-art and data streams, and replaces
        the original file. Edit if you want, or save as-is.
      </p>
      <div style={{ display: "flex", gap: 8, alignItems: "center", marginBottom: 8 }}>
        <label style={{ fontSize: 12, color: "var(--text-dim)" }}>name</label>
        <input
          value={name}
          onChange={e => setName(e.target.value)}
          style={{ flex: 1 }}
        />
      </div>
      <textarea
        value={yaml}
        onChange={e => setYaml(e.target.value)}
        spellCheck={false}
        style={{
          width: "100%",
          minHeight: 280,
          fontFamily: "var(--font-mono)",
          fontSize: 12,
          background: "var(--surface)",
          color: "var(--text)",
          border: "1px solid var(--border)",
          borderRadius: "var(--r-2)",
          padding: 8,
          resize: "vertical",
        }}
      />
      {error && (
        <p className="hint" style={{ color: "var(--bad)" }}>{error}</p>
      )}
      <div style={{ display: "flex", gap: 8, marginTop: 12 }}>
        <button onClick={() => create.mutate()} disabled={disabled}>
          Save flow
        </button>
        <button className="btn-ghost" onClick={onSkip}>Skip this step</button>
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Wire into `setup-wizard.tsx`**

Add the import at the top:

```tsx
import FlowStep from "./setup-wizard-steps/flow";
```

Update the Plugins step's wiring to advance to `flow`:

```tsx
{step === "plugins" && (
  <PluginsStep
    onContinue={() => next("flow")}
    onSkip={() => next("flow")}
  />
)}
{step === "flow" && (
  <FlowStep
    onCreated={() => next("done")}
    onSkip={() => next("done")}
  />
)}
```

- [ ] **Step 5: Build + end-to-end smoke test**

```bash
npm --prefix web run build 2>&1 | tail -5
```

Smoke test (full wizard, fresh data dir):
- Open `/dashboard` → wizard appears.
- Welcome → Start.
- Source step → add a webhook source (e.g. `kind=webhook, name=test, secret_token=anything`).
- Notifier step → skip.
- Plugins step → Continue.
- Flow step → click Save flow.
- Done → click Finish.
- Visit `/sources`, `/flows` — confirm the new source and flow rows are there.
- Refresh browser → wizard does NOT reappear.

- [ ] **Step 6: Commit**

```bash
test "$(git branch --show-current)" = "feat/setup-wizard" || { echo "WRONG BRANCH"; exit 1; }
git add web/src/templates/hevc-normalize.yaml \
        web/src/components/setup-wizard-steps/flow.tsx \
        web/src/components/setup-wizard.tsx
git commit -m "web: wizard step — Flow + hevc-normalize template"
```

---

## Self-Review

**Spec coverage:**
- "Auto-launch on first visit when no sources exist AND wizard.completed != true" → Task 3 (gating in `setup-wizard.tsx`).
- "Six views: Welcome / Source / Notifier / Plugins / Flow / Done" → Task 3 (Welcome + Done) plus Tasks 4-7 (each adds one).
- "Skip wizard / Finish PATCH wizard.completed = true" → Task 3 (`markDone` mutation called from both `Skip wizard` and the Done step's Finish button).
- "Reuse existing forms by factoring" → Tasks 1, 2 (`AddSourceForm`, `AddNotifierForm`); reused by the Sources / Notifiers pages AND by the wizard.
- "Flow step pre-fills hevc-normalize" → Task 7 (`web/src/templates/hevc-normalize.yaml` + `?raw` import).
- "Plugins step is informational, no in-wizard install" → Task 6 (text + Continue/Skip).
- "Modal styling reuses .modal-* CSS classes" → Task 3 wraps the wizard in `<div className="modal-backdrop"><div className="modal wizard-modal">` plus adds `.wizard-*` for the inner two-column layout.
- "Two-column layout with numbered rail + content + footer" → Task 3 (`.wizard-rail` + `.wizard-pane`).
- "No backend changes" → confirmed; only frontend edits.
- "Tests" → spec mentions vitest tests; reality is the web tree has no test infrastructure. Plan calls this out at the top under Tests, drops automated tests, relies on manual smoke checks at each task.

**Placeholder scan:** no TBD/TODO/"add appropriate". Every step block has runnable code or an exact instruction. The Notifiers page edit in Task 2 Step 2 describes the change rather than showing the full diff because the page is large and the change is mechanical (drop unused state + the add-form surface div, replace with `<AddNotifierForm />`); the implementer reads the file first and applies the diff.

**Type consistency:** `Step`, `STEP_ORDER`, `STEP_LABELS` defined Task 3, used Tasks 4-7 unchanged. `AddSourceForm` / `AddNotifierForm` props (`onCreated?: (id: number) => void`) consistent across creation (Tasks 1, 2) and consumption (Tasks 4, 5).
