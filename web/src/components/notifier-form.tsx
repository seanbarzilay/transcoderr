import type { ChangeEvent } from "react";

export const KINDS = ["discord", "jellyfin", "ntfy", "telegram", "webhook"] as const;
export type Kind = (typeof KINDS)[number];

type ScalarField = {
  name: string;
  label: string;
  type: "text" | "password";
  required: boolean;
  placeholder?: string;
  hint?: string;
};

type PathMappingsField = {
  name: "path_mappings";
  type: "path_mappings";
  label: string;
  hint?: string;
};

type Field = ScalarField | PathMappingsField;

/// Per-kind field layout. Single source of truth: drives form rendering,
/// initial value seeding, and the JSON-config build on submit.
const SCHEMAS: Record<Kind, Field[]> = {
  discord: [
    {
      name: "url",
      label: "Webhook URL",
      type: "text",
      required: true,
      placeholder: "https://discord.com/api/webhooks/...",
    },
  ],
  jellyfin: [
    {
      name: "url",
      label: "Server URL",
      type: "text",
      required: true,
      placeholder: "http://jellyfin:8096",
    },
    {
      name: "api_key",
      label: "API key",
      type: "password",
      required: true,
      hint: "Dashboard → API Keys (must be admin-equivalent)",
    },
    {
      name: "path_mappings",
      type: "path_mappings",
      label: "Path mappings",
      hint:
        "Required when transcoderr and Jellyfin run with different mounts. " +
        "First prefix that matches the file path wins.",
    },
  ],
  ntfy: [
    {
      name: "server",
      label: "Server",
      type: "text",
      required: false,
      placeholder: "https://ntfy.sh",
    },
    {
      name: "topic",
      label: "Topic",
      type: "text",
      required: true,
      placeholder: "my-topic",
    },
  ],
  telegram: [
    {
      name: "bot_token",
      label: "Bot token",
      type: "password",
      required: true,
      placeholder: "123456:ABC-DEF...",
    },
    {
      name: "chat_id",
      label: "Chat ID",
      type: "text",
      required: true,
      placeholder: "-1001234567890",
    },
  ],
  webhook: [
    {
      name: "url",
      label: "URL",
      type: "text",
      required: true,
      placeholder: "https://example.com/hook",
    },
  ],
};

export type PathMapping = { from: string; to: string };
export type FormValue = Record<string, string | PathMapping[]>;

/// Build an empty form value for a kind. Scalar fields default to ""; the
/// path_mappings field defaults to []. Used by the Add card.
export function emptyForm(kind: Kind): FormValue {
  const out: FormValue = {};
  for (const f of SCHEMAS[kind]) {
    out[f.name] = f.type === "path_mappings" ? [] : "";
  }
  return out;
}

/// Seed a form value from an existing notifier config (Edit row). For
/// password fields whose API value is the redacted sentinel `***`, the
/// form starts empty so the user can either leave it blank ("keep
/// current secret") or type a replacement.
export function fromConfig(kind: Kind, config: any): FormValue {
  const out: FormValue = {};
  for (const f of SCHEMAS[kind]) {
    const incoming = config?.[f.name];
    if (f.type === "path_mappings") {
      out[f.name] = Array.isArray(incoming)
        ? incoming
            .filter((m: any) => m && typeof m === "object")
            .map((m: any) => ({ from: String(m.from ?? ""), to: String(m.to ?? "") }))
        : [];
    } else if (f.type === "password") {
      out[f.name] = incoming === "***" ? "" : String(incoming ?? "");
    } else {
      out[f.name] = String(incoming ?? "");
    }
  }
  return out;
}

/// Produce the JSON config payload from a form value. On edit, an empty
/// password field is sent as `"***"` so the server's unredact pass keeps
/// the current secret. Empty optional scalars are omitted entirely so
/// notifiers like ntfy fall back to their defaults.
export function toConfig(kind: Kind, value: FormValue, isEdit: boolean): any {
  const out: any = {};
  for (const f of SCHEMAS[kind]) {
    const v = value[f.name];
    if (f.type === "path_mappings") {
      const mappings = (v as PathMapping[])
        .map(m => ({ from: m.from.trim(), to: m.to.trim() }))
        .filter(m => m.from && m.to);
      if (mappings.length > 0) out.path_mappings = mappings;
      continue;
    }
    const s = (v as string) ?? "";
    if (f.type === "password") {
      if (s === "" && isEdit) {
        out[f.name] = "***";
      } else if (s !== "") {
        out[f.name] = s;
      }
      continue;
    }
    if (s !== "") out[f.name] = s;
  }
  return out;
}

/// Returns null if the form is submittable, or a user-facing reason if not.
export function validate(kind: Kind, value: FormValue, isEdit: boolean): string | null {
  for (const f of SCHEMAS[kind]) {
    if (f.type === "path_mappings") continue;
    if (!f.required) continue;
    const s = (value[f.name] as string) ?? "";
    // On edit, an empty password is "keep current" — that's allowed.
    if (s === "" && !(isEdit && f.type === "password")) {
      return `${f.label} is required`;
    }
  }
  return null;
}

interface Props {
  kind: Kind;
  value: FormValue;
  onChange: (next: FormValue) => void;
  isEdit: boolean;
  idPrefix: string;
}

export default function NotifierForm({ kind, value, onChange, isEdit, idPrefix }: Props) {
  const fields = SCHEMAS[kind];
  const set = (name: string, v: string | PathMapping[]) =>
    onChange({ ...value, [name]: v });

  return (
    <div className="notifier-form">
      {fields.map(f => {
        const id = `${idPrefix}-${f.name}`;
        if (f.type === "path_mappings") {
          const mappings = (value[f.name] as PathMapping[]) ?? [];
          return (
            <div key={f.name} className="notifier-form-row">
              <label className="notifier-form-label" htmlFor={id}>
                {f.label}
              </label>
              <div className="notifier-form-mappings" id={id}>
                {mappings.map((m, i) => (
                  <div key={i} className="notifier-form-mapping-row">
                    <input
                      placeholder="from (transcoderr path)"
                      value={m.from}
                      onChange={(e: ChangeEvent<HTMLInputElement>) =>
                        set(
                          f.name,
                          mappings.map((mm, j) =>
                            j === i ? { ...mm, from: e.target.value } : mm
                          )
                        )
                      }
                    />
                    <span className="notifier-form-mapping-arrow">→</span>
                    <input
                      placeholder="to (jellyfin path)"
                      value={m.to}
                      onChange={(e: ChangeEvent<HTMLInputElement>) =>
                        set(
                          f.name,
                          mappings.map((mm, j) =>
                            j === i ? { ...mm, to: e.target.value } : mm
                          )
                        )
                      }
                    />
                    <button
                      type="button"
                      className="btn-ghost notifier-form-mapping-remove"
                      onClick={() =>
                        set(f.name, mappings.filter((_, j) => j !== i))
                      }
                      aria-label="Remove mapping"
                    >
                      ✕
                    </button>
                  </div>
                ))}
                <button
                  type="button"
                  className="btn-ghost"
                  onClick={() => set(f.name, [...mappings, { from: "", to: "" }])}
                >
                  + Add mapping
                </button>
                {f.hint && <div className="notifier-form-hint">{f.hint}</div>}
              </div>
            </div>
          );
        }
        return (
          <div key={f.name} className="notifier-form-row">
            <label className="notifier-form-label" htmlFor={id}>
              {f.label}
              {f.required && !isEdit && <span className="notifier-form-required"> *</span>}
            </label>
            <div className="notifier-form-input">
              <input
                id={id}
                type={f.type === "password" ? "password" : "text"}
                placeholder={
                  isEdit && f.type === "password"
                    ? "leave blank to keep current"
                    : f.placeholder
                }
                value={(value[f.name] as string) ?? ""}
                onChange={(e: ChangeEvent<HTMLInputElement>) => set(f.name, e.target.value)}
                autoComplete={f.type === "password" ? "new-password" : "off"}
              />
              {f.hint && <div className="notifier-form-hint">{f.hint}</div>}
            </div>
          </div>
        );
      })}
    </div>
  );
}
