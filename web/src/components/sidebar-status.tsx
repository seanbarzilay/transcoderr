import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { useLive } from "../state/live";
import { api } from "../api/client";

/// Last path component of a possibly-Windows path. Used to keep the
/// hover popover compact — full paths in /runs/:id when the operator
/// clicks through.
function basename(p: string | undefined): string {
  if (!p) return "—";
  const s = p.replace(/\\/g, "/");
  const i = s.lastIndexOf("/");
  return i >= 0 ? s.slice(i + 1) : s;
}

function clampPct(n: number): number {
  return Math.max(0, Math.min(100, n));
}

const FFMPEG_PREVIEW_MAX = 80;
/// How long the popover auto-stays-open after a job transitions to
/// running, before reverting to hover-only.
const AUTO_OPEN_MS = 4000;

export default function SidebarStatus() {
  const queue = useLive((s) => s.queue);
  const jobProgress = useLive((s) => s.jobProgress);
  const nav = useNavigate();

  const version = useQuery({
    queryKey: ["version"],
    queryFn: () => api.version(),
    staleTime: Infinity,
  });

  // Source-of-truth for "what's running right now". Reading from the
  // API survives missed-SSE-events (page reload mid-job; the
  // JobState transition already fired and we'd never see it via the
  // live stream). Polled while the queue says we have running work.
  const runningRuns = useQuery({
    queryKey: ["sidebar-status", "running"],
    queryFn: () => api.runs.list({ status: "running", limit: 10 }),
    enabled: queue.running > 0,
    refetchInterval: queue.running > 0 ? 2000 : false,
    staleTime: 0,
  });
  const running = runningRuns.data ?? [];

  const isLive = queue.running > 0;
  const isPending = queue.pending > 0;
  const isReactive = isLive || isPending;

  // Auto-open the popover for AUTO_OPEN_MS when a job transitions to
  // running so the operator gets a peek-what-just-started without
  // having to hover. Once the timer fires the popover behaves as
  // hover-only again.
  const [autoOpen, setAutoOpen] = useState(false);
  const prevRunning = useRef<number>(queue.running);
  useEffect(() => {
    const prev = prevRunning.current;
    prevRunning.current = queue.running;
    if (queue.running > prev) {
      setAutoOpen(true);
      const t = setTimeout(() => setAutoOpen(false), AUTO_OPEN_MS);
      return () => clearTimeout(t);
    }
  }, [queue.running]);

  /// Click target: most-recent running job's detail page; falls back
  /// to /runs filters when nothing is running but we're still
  /// reactive (pending-only state).
  const onClick = () => {
    if (running.length > 0) {
      nav(`/runs/${running[0].id}`);
    } else if (isPending) {
      nav(`/runs?status=pending`);
    } else {
      nav(`/runs`);
    }
  };

  return (
    <div
      className={
        "sidebar-status" +
        (isLive ? " is-live" : "") +
        (!isLive && isPending ? " is-pending" : "") +
        (isReactive ? " is-reactive" : "") +
        (autoOpen ? " is-auto-open" : "")
      }
      onClick={isReactive ? onClick : undefined}
      role={isReactive ? "button" : undefined}
      tabIndex={isReactive ? 0 : -1}
      onKeyDown={(e) => {
        if (!isReactive) return;
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onClick();
        }
      }}
      aria-label={
        isLive
          ? running.length === 1
            ? `Open running job`
            : `Open running jobs`
          : isPending
          ? `Open pending queue`
          : undefined
      }
    >
      <div className="sidebar-status-bar">
        <div>
          {isLive && <span className="sidebar-status-dot" aria-hidden="true" />}
          Queue <span className="dim">{queue.pending}</span>
          {"  "}·{"  "}
          Running <span className="dim">{queue.running}</span>
        </div>
        <div className="muted">
          {version.data ? `v${version.data.version}` : ""}
        </div>
      </div>

      {isReactive && (
        <div className="sidebar-status-popover" role="tooltip">
          {running.length > 0 ? (
            running.map((run) => {
              const prog = jobProgress[run.id];
              const pctNum =
                typeof prog?.pct === "number" ? clampPct(prog.pct) : null;
              const ff =
                prog?.lastFfmpegLine && prog.lastFfmpegLine.length > FFMPEG_PREVIEW_MAX
                  ? prog.lastFfmpegLine.slice(0, FFMPEG_PREVIEW_MAX) + "…"
                  : prog?.lastFfmpegLine;
              return (
                <div key={run.id} className="sidebar-status-job">
                  <div className="sidebar-status-job-title mono">
                    #{run.id} · {basename(run.file_path)}
                  </div>
                  {pctNum != null && (
                    <div className="sidebar-status-job-bar">
                      <div
                        className="sidebar-status-job-bar-fill"
                        style={{ width: `${pctNum}%` }}
                      />
                    </div>
                  )}
                  <div className="sidebar-status-job-meta">
                    {prog?.lastStepId ?? "starting…"}
                    {pctNum != null ? ` · ${pctNum.toFixed(0)}%` : ""}
                  </div>
                  {ff && (
                    <div className="sidebar-status-job-ff mono">{ff}</div>
                  )}
                </div>
              );
            })
          ) : isPending ? (
            <div className="sidebar-status-job-meta">
              {queue.pending} pending in queue
            </div>
          ) : (
            <div className="sidebar-status-job-meta">starting…</div>
          )}
          <div className="sidebar-status-hint">click to open →</div>
        </div>
      )}
    </div>
  );
}
