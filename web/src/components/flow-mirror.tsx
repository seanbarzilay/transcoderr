export default function FlowMirror({ parsed }: { parsed: any }) {
  if (!parsed) return null;
  return (
    <div>
      <div style={{ fontWeight: 600 }}>{parsed.name}</div>
      <Steps nodes={parsed.steps ?? []} />
    </div>
  );
}

function Steps({ nodes }: { nodes: any[] }) {
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

function describe(n: any): string {
  if (n.use  != null) return `\u25b6 ${n.use}${n.id ? ` (${n.id})` : ""}`;
  if (n.if   != null) return `? if ${n.if}`;
  if (n.return != null) return `\u2190 return ${n.return}`;
  return JSON.stringify(n);
}
