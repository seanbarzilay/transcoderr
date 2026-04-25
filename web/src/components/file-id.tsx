/// FileId — displays a run's input file as `basename` (truncated if long)
/// with the full path on hover.

export function basename(path: string): string {
  const last = path.split(/[\\/]/).filter(Boolean).pop();
  return last ?? path;
}

export default function FileId({
  path,
  width,
}: {
  path: string;
  width?: number | string;
}) {
  if (!path) return <span className="muted">—</span>;
  return (
    <span
      title={path}
      className="mono"
      style={{
        display: "inline-block",
        maxWidth: width ?? 360,
        overflow: "hidden",
        textOverflow: "ellipsis",
        whiteSpace: "nowrap",
        verticalAlign: "bottom",
        fontSize: 12,
      }}
    >
      {basename(path)}
    </span>
  );
}
