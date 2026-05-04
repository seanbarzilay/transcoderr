import { useEffect, useRef, useState } from "react";
import { api } from "../api/client";
import { errorMessage } from "../lib/errors";
import { asRecord } from "../lib/records";

type Line = { kind: "status" | "stdout" | "stderr"; text: string };
type Result =
  | { state: "running" }
  | { state: "done"; installed: string }
  | { state: "error"; message: string };

interface Props {
  catalogId: number;
  name: string;
  onClose: () => void;
}

/**
 * Streams `/plugin-catalog-entries/:cat/:name/install` and renders each
 * SSE event as a line. Disables the close button until the stream
 * reaches a terminal `done` or `error` event so the operator doesn't
 * dismiss mid-pip-download. The server keeps installing even if they
 * close their tab anyway, but discouraging the click avoids confusion.
 */
export default function InstallLogModal({ catalogId, name, onClose }: Props) {
  const [lines, setLines] = useState<Line[]>([]);
  const [result, setResult] = useState<Result>({ state: "running" });
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        for await (const { event, data } of api.plugins.installStream(catalogId, name)) {
          if (cancelled) return;
          const record = asRecord(data);
          if (event === "status") {
            setLines((ls) => [...ls, { kind: "status", text: String(record.message ?? "") }]);
          } else if (event === "log") {
            setLines((ls) => [
              ...ls,
              { kind: record.stream === "stderr" ? "stderr" : "stdout", text: String(record.line ?? "") },
            ]);
          } else if (event === "done") {
            setResult({ state: "done", installed: String(record.installed ?? name) });
          } else if (event === "error") {
            setResult({ state: "error", message: String(record.message ?? "Install failed") });
          }
        }
      } catch (e: unknown) {
        if (!cancelled) setResult({ state: "error", message: errorMessage(e) });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [catalogId, name]);

  // Auto-scroll the log to the bottom as new lines arrive.
  useEffect(() => {
    if (scrollRef.current) scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
  }, [lines.length]);

  const terminal = result.state !== "running";

  return (
    <div className="modal-backdrop" onClick={terminal ? onClose : undefined}>
      <div className="modal install-log-modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h3>Install <span className="mono">{name}</span></h3>
          <button
            className="btn-text"
            onClick={onClose}
            disabled={!terminal}
            title={terminal ? "Close" : "Install in progress; the server will continue even if you close this dialog."}
          >
            ✕
          </button>
        </div>

        <div ref={scrollRef} className="install-log-body">
          {lines.length === 0 && result.state === "running" && (
            <div className="muted">Connecting...</div>
          )}
          {lines.map((l, i) => (
            <div key={i} className={`install-log-line install-log-${l.kind}`}>
              {l.kind === "status" ? (
                <span><span className="dim">▸</span> {l.text}</span>
              ) : (
                <span className="mono">{l.text}</span>
              )}
            </div>
          ))}
        </div>

        <div className="modal-footer">
          {result.state === "running" && (
            <span className="muted"><span className="dot live" /> Installing — keep this tab open to watch logs (server will continue if you close).</span>
          )}
          {result.state === "done" && (
            <span className="ok">✓ Installed <span className="mono">{result.installed}</span></span>
          )}
          {result.state === "error" && (
            <span className="bad">✗ {result.message}</span>
          )}
        </div>
      </div>
    </div>
  );
}
