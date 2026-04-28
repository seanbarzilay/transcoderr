import type { ReactNode } from "react";

interface Item {
  id: number;
  title: string;
  year: number | null;
  poster_url: string | null;
  has_file?: boolean;
}

interface Props {
  items: Item[];
  onSelect: (id: number) => void;
  selectedId: number | null;
  renderBadge?: (item: Item) => ReactNode;
}

export default function PosterGrid({ items, onSelect, selectedId, renderBadge }: Props) {
  if (items.length === 0) {
    return <div className="empty">No matches.</div>;
  }
  return (
    <div className="poster-grid">
      {items.map((it) => (
        <button
          type="button"
          key={it.id}
          className={"poster-card" + (selectedId === it.id ? " is-selected" : "")}
          onClick={() => onSelect(it.id)}
        >
          {it.poster_url ? (
            <img className="poster-img" src={it.poster_url} alt={it.title} loading="lazy" />
          ) : (
            <div className="poster-img poster-img-placeholder">🎬</div>
          )}
          <div className="poster-meta">
            <div className="poster-title">{it.title}</div>
            <div className="poster-sub">
              {it.year ?? ""}
              {renderBadge?.(it)}
            </div>
          </div>
        </button>
      ))}
    </div>
  );
}
