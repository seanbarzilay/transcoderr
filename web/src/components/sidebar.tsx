import { NavLink } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../api/client";
import { useLive } from "../state/live";

const links: [string, string][] = [
  ["/dashboard", "Dashboard"],
  ["/flows", "Flows"],
  ["/runs", "Runs"],
];

const config: [string, string][] = [
  ["/sources", "Sources"],
  ["/notifiers", "Notifiers"],
  ["/plugins", "Plugins"],
  ["/settings", "Settings"],
];

export default function Sidebar() {
  const queue = useLive((s) => s.queue);
  const version = useQuery({
    queryKey: ["version"],
    queryFn: () => api.version(),
    staleTime: Infinity,
  });
  return (
    <nav className="sidebar">
      <div className="brand">
        <span className="brand-dot" />
        <span>transcoder<span className="brand-x">/r</span></span>
      </div>

      <div className="nav">
        <div className="nav-section">Operate</div>
        {links.map(([href, label]) => (
          <NavLink
            key={href}
            to={href}
            className={({ isActive }) =>
              "nav-link" + (isActive ? " is-active" : "")
            }
          >
            {label}
          </NavLink>
        ))}

        <div className="nav-section">Configure</div>
        {config.map(([href, label]) => (
          <NavLink
            key={href}
            to={href}
            className={({ isActive }) =>
              "nav-link" + (isActive ? " is-active" : "")
            }
          >
            {label}
          </NavLink>
        ))}
      </div>

      <div className="sidebar-foot">
        <div>
          Queue <span className="dim">{queue.pending}</span>
          {"  "}·{"  "}
          Running <span className="dim">{queue.running}</span>
        </div>
        <div className="muted">{version.data ? `v${version.data.version}` : ""}</div>
      </div>
    </nav>
  );
}
