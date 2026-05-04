import type { ReactNode } from "react";

interface Props {
  open: boolean;
  onClose: () => void;
  children: ReactNode;
}

export default function DetailPanel({ open, onClose, children }: Props) {
  if (!open) return null;
  return (
    <>
      <div className="detail-scrim" onClick={onClose} />
      <aside className="detail-panel">
        <button
          type="button"
          className="detail-close"
          onClick={onClose}
          aria-label="Close detail panel"
        >
          ×
        </button>
        <div className="detail-body">{children}</div>
      </aside>
    </>
  );
}

export function FileSummaryRow({
  label,
  value,
}: {
  label: string;
  value: string | null;
}) {
  if (!value) return null;
  return (
    <div className="detail-row">
      <span className="detail-label">{label}</span>
      <span className="detail-value">{value}</span>
    </div>
  );
}
