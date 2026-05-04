import type { ChangeEvent } from "react";
import {
  SCHEMAS,
  type FormValue,
  type Kind,
  type PathMapping,
  type PathMappingsField,
  type ScalarField,
} from "../lib/notifier-form";

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
