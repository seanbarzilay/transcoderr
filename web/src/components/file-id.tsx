import { basename } from "../lib/path";

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
