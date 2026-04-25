import { NavLink } from "react-router-dom";

const links: [string, string][] = [
  ["/dashboard", "Dashboard"],
  ["/flows", "Flows"],
  ["/runs", "Runs"],
  ["/sources", "Sources"],
  ["/notifiers", "Notifiers"],
  ["/plugins", "Plugins"],
  ["/settings", "Settings"],
];

export default function Sidebar() {
  return (
    <nav style={{ width: 200, background: "rgba(255,255,255,0.04)", padding: 16, borderRight: "1px solid rgba(255,255,255,0.08)" }}>
      <h3 style={{ marginTop: 0 }}>transcoderr</h3>
      <ul style={{ listStyle: "none", padding: 0 }}>
        {links.map(([href, label]) => (
          <li key={href} style={{ marginBottom: 8 }}>
            <NavLink to={href} style={({ isActive }) => ({
              color: isActive ? "#fff" : "rgba(255,255,255,0.7)",
              textDecoration: "none",
              fontWeight: isActive ? 600 : 400,
            })}>
              {label}
            </NavLink>
          </li>
        ))}
      </ul>
    </nav>
  );
}
