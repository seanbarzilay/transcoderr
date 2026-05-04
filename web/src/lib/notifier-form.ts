import { asRecord } from "./records";

export const KINDS = ["discord", "jellyfin", "ntfy", "telegram", "webhook"] as const;
export type Kind = (typeof KINDS)[number];

export type ScalarField = {
  name: string;
  type: "text" | "password";
  required: boolean;
  placeholder: string;
  grow?: number;
};

export type PathMappingsField = {
  name: "path_mappings";
  type: "path_mappings";
  label: string;
  hint?: string;
};

export type Field = ScalarField | PathMappingsField;

export const SCHEMAS: Record<Kind, Field[]> = {
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
      placeholder: "api key (Dashboard -> API Keys)",
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

export function fromConfig(kind: Kind, config: unknown): FormValue {
  const record = asRecord(config);
  const out: FormValue = {};
  for (const f of SCHEMAS[kind]) {
    const incoming = record[f.name];
    if (f.type === "path_mappings") {
      out[f.name] = Array.isArray(incoming)
        ? incoming
            .filter((m: unknown) => m != null && typeof m === "object")
            .map((m: unknown) => {
              const mapping = asRecord(m);
              return { from: String(mapping.from ?? ""), to: String(mapping.to ?? "") };
            })
        : [];
    } else if (f.type === "password") {
      out[f.name] = incoming === "***" ? "" : String(incoming ?? "");
    } else {
      out[f.name] = String(incoming ?? "");
    }
  }
  return out;
}

export function toConfig(kind: Kind, value: FormValue, isEdit: boolean): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const f of SCHEMAS[kind]) {
    const v = value[f.name];
    if (f.type === "path_mappings") {
      const mappings = (v as PathMapping[])
        .map((m) => ({ from: m.from.trim(), to: m.to.trim() }))
        .filter((m) => m.from && m.to);
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
