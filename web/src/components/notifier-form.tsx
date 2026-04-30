import type { ChangeEvent } from "react";

export const KINDS = ["discord", "jellyfin", "ntfy", "telegram", "webhook"] as const;
export type Kind = (typeof KINDS)[number];

type ScalarField = {
  name: string;
  type: "text" | "password";
  required: boolean;
  placeholder: string;
  /// Optional flex-grow weight inside the row. Defaults to 1.
  grow?: number;
};

type PathMappingsField = {
  name: "path_mappings";
  type: "path_mappings";
  label: string;
  hint?: string;
};

type Field = ScalarField | PathMappingsField;

const SCHEMAS: Record<Kind, Field[]> = {
  discord: [
    {
      name: "url",
      type: "text",
      required: true,
      placeholder: "discord webhook url",
      grow: 4,
    },
  ],
  jellyfin: [
    {
      name: "url",
      type: "text",
      required: true,
      placeholder: "jellyfin url (e.g. http://jellyfin:8096)",
      grow: 3,
    },
    {
      name: "api_key",
      type: "password",
      required: true,
      placeholder: "api key (Dashboard → API Keys)",
      grow: 2,
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
      type: "text",
      required: false,
      placeholder: "server (default https://ntfy.sh)",
      grow: 2,
    },
    {
      name: "topic",
      type: "text",
      required: true,
      placeholder: "topic",
      grow: 2,
    },
  ],
  telegram: [
    {
      name: "bot_token",
      type: "password",
      required: true,
      placeholder: "bot token (e.g. 123:ABC...)",
      grow: 3,
    },
    {
      name: "chat_id",
      type: "text",
      required: true,
      placeholder: "chat id (e.g. -1001234567890)",
      grow: 2,
    },
  ],
  webhook: [
    {
      name: "url",
      type: "text",
      required: true,
      placeholder: "https://example.com/hook",
      grow: 4,
    },
  ],
};

export type PathMapping = { from: string; to: string };
export type FormValue = Record<string, string | PathMapping[]>;

export function emptyForm(kind: Kind): FormValue {
  const out: FormValue = {};
  for (const f of SCHEMAS[kind]) {
    out[f.name] = f.type === "path_mappings" ? [] : "";
  }
  return out;
}

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

export function validate(kind: Kind, value: FormValue, isEdit: boolean): string | null {
  for (const f of SCHEMAS[kind]) {
    if (f.type === "path_mappings") continue;
    if (!f.required) continue;
    const s = (value[f.name] as string) ?? "";
    if (s === "" && !(isEdit && f.type === "password")) {
      return `${f.placeholder} is required`;
    }
  }
  return null;
}

interface Props {
  kind: Kind;
  value: FormValue;
  onChange: (next: FormValue) => void;
  isEdit: boolean;
}

export default function NotifierForm({ kind, value, onChange, isEdit }: Props) {
  const fields = SCHEMAS[kind];
  const set = (name: string, v: string | PathMapping[]) =>
    onChange({ ...value, [name]: v });

  const scalarFields = fields.filter((f): f is ScalarField => f.type !== "path_mappings");
  const mappingField = fields.find(
    (f): f is PathMappingsField => f.type === "path_mappings"
  );

  return (
    <div className="notifier-form">
      {scalarFields.length > 0 && (
        <div className="notifier-form-fields">
          {scalarFields.map(f => (
            <input
              key={f.name}
              type={f.type === "password" ? "password" : "text"}
              placeholder={
                isEdit && f.type === "password"
                  ? "leave blank to keep current"
                  : f.placeholder
              }
              value={(value[f.name] as string) ?? ""}
              onChange={(e: ChangeEvent<HTMLInputElement>) => set(f.name, e.target.value)}
              autoComplete={f.type === "password" ? "new-password" : "off"}
              style={{ flex: f.grow ?? 1, minWidth: 180 }}
            />
          ))}
        </div>
      )}

      {mappingField && (
        <PathMappingsEditor
          label={mappingField.label}
          hint={mappingField.hint}
          mappings={(value[mappingField.name] as PathMapping[]) ?? []}
          onChange={v => set(mappingField.name, v)}
        />
      )}
    </div>
  );
}

interface PMProps {
  label: string;
  hint?: string;
  mappings: PathMapping[];
  onChange: (next: PathMapping[]) => void;
}

function PathMappingsEditor({ label, hint, mappings, onChange }: PMProps) {
  return (
    <div className="notifier-form-mappings-block">
      <div className="label">{label}</div>
      {mappings.map((m, i) => (
        <div key={i} className="notifier-form-mapping-row">
          <input
            placeholder="from (transcoderr path)"
            value={m.from}
            onChange={(e: ChangeEvent<HTMLInputElement>) =>
              onChange(
                mappings.map((mm, j) => (j === i ? { ...mm, from: e.target.value } : mm))
              )
            }
          />
          <span className="notifier-form-mapping-arrow">→</span>
          <input
            placeholder="to (jellyfin path)"
            value={m.to}
            onChange={(e: ChangeEvent<HTMLInputElement>) =>
              onChange(
                mappings.map((mm, j) => (j === i ? { ...mm, to: e.target.value } : mm))
              )
            }
          />
          <button
            type="button"
            className="btn-ghost notifier-form-mapping-remove"
            onClick={() => onChange(mappings.filter((_, j) => j !== i))}
            aria-label="Remove mapping"
          >
            ✕
          </button>
        </div>
      ))}
      <div className="notifier-form-mapping-actions">
        <button
          type="button"
          className="btn-ghost"
          onClick={() => onChange([...mappings, { from: "", to: "" }])}
        >
          + Add mapping
        </button>
        {hint && <span className="notifier-form-hint">{hint}</span>}
      </div>
    </div>
  );
}
