import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../../api/client";
import { errorMessage } from "../../lib/errors";
import {
  KINDS,
  emptyForm,
  toConfig,
  validate,
} from "../../lib/notifier-form";
import type { Kind, FormValue } from "../../lib/notifier-form";
import NotifierForm from "../notifier-form";

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
    onError: (e: unknown) => setError(errorMessage(e, "create failed")),
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
