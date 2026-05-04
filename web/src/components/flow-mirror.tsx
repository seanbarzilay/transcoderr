type FlowNode = {
  id?: string;
  use?: string;
  if?: string;
  return?: string;
  then?: FlowNode[];
  else?: FlowNode[];
};

type ParsedFlow = {
  name?: string;
  steps?: FlowNode[];
};

export default function FlowMirror({ parsed }: { parsed: unknown }) {
  if (!isParsedFlow(parsed)) return null;
  return (
    <div>
      <div style={{ fontWeight: 600 }}>{parsed.name}</div>
      <Steps nodes={parsed.steps ?? []} />
    </div>
  );
}

function isParsedFlow(value: unknown): value is ParsedFlow {
  return value != null && typeof value === "object";
}

function Steps({ nodes }: { nodes: FlowNode[] }) {
  return (
    <ul style={{ paddingLeft: 16 }}>
      {nodes.map((n, i) => (
        <li key={i} style={{ marginBottom: 4 }}>
          {describe(n)}
          {n.then && <Steps nodes={n.then} />}
          {n.else && <Steps nodes={n.else} />}
        </li>
      ))}
    </ul>
  );
}

function describe(n: FlowNode): string {
  if (n.use  != null) return `\u25b6 ${n.use}${n.id ? ` (${n.id})` : ""}`;
  if (n.if   != null) return `? if ${n.if}`;
  if (n.return != null) return `\u2190 return ${n.return}`;
  return JSON.stringify(n);
}
