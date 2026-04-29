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

export default function SidebarStatus() {
  const queue = useLive((s) => s.queue);
  const jobStatus = useLive((s) => s.jobStatus);
  const jobProgress = useLive((s) => s.jobProgress);
  const nav = useNavigate();

  const version = useQuery({
    queryKey: ["version"],
    queryFn: () => api.version(),
    staleTime: Infinity,
  });

  // Job ids that the live stream currently reports as running. Falls
  // back on `queue.running` for the count when no JobState events have
  // come through yet (e.g. just-started worker).
  const runningIds = Object.entries(jobStatus)
    .filter(([_, v]) => v.status === "running")
    .map(([id]) => Number(id))
    .sort((a, b) => b - a); // newest first

  const isLive = runningIds.length > 0 || queue.running > 0;
  const isPending = queue.pending > 0;
  const isReactive = isLive || isPending;

  /// Where the click takes the user. Picks the most specific destination:
  /// single-running-job → that job's detail; multi-running → the runs
  /// list filtered to running; pending-only → filtered to pending; else
  /// the unfiltered runs list.
  const onClick = () => {
    if (runningIds.length === 1) {
      nav(`/runs/${runningIds[0]}`);
    } else if (runningIds.length > 1) {
      nav(`/runs?status=running`);
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
        (isReactive ? " is-reactive" : "")
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
          ? `Open running job` + (runningIds.length === 1 ? `` : `s`)
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
          {runningIds.length > 0 ? (
            runningIds.map((id) => {
              const label = jobStatus[id]?.label;
              const prog = jobProgress[id];
              const pctNum =
                typeof prog?.pct === "number" ? clampPct(prog.pct) : null;
              const ff =
                prog?.lastFfmpegLine && prog.lastFfmpegLine.length > FFMPEG_PREVIEW_MAX
                  ? prog.lastFfmpegLine.slice(0, FFMPEG_PREVIEW_MAX) + "…"
                  : prog?.lastFfmpegLine;
              return (
                <div key={id} className="sidebar-status-job">
                  <div className="sidebar-status-job-title mono">
                    #{id} · {basename(label)}
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
                    {prog?.lastStepId ?? ""}
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
          ) : null}
          <div className="sidebar-status-hint">click to open →</div>
        </div>
      )}
    </div>
  );
}
