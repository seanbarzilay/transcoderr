import { NavLink } from "react-router-dom";
import SidebarStatus from "./sidebar-status";

const links: [string, string][] = [
  ["/dashboard", "Dashboard"],
  ["/flows", "Flows"],
  ["/runs", "Runs"],
  ["/radarr", "Browse Radarr"],
  ["/sonarr", "Browse Sonarr"],
];

const config: [string, string][] = [
  ["/sources", "Sources"],
  ["/notifiers", "Notifiers"],
  ["/plugins", "Plugins"],
  ["/settings", "Settings"],
];

interface Props {
  open?: boolean;
  onNavigate?: () => void;
}

export default function Sidebar({ open = false, onNavigate }: Props) {
  return (
    <nav className={"sidebar" + (open ? " is-open" : "")}>
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
            onClick={onNavigate}
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
            onClick={onNavigate}
            className={({ isActive }) =>
              "nav-link" + (isActive ? " is-active" : "")
            }
          >
            {label}
          </NavLink>
        ))}
      </div>

      <SidebarStatus />
    </nav>
  );
}
