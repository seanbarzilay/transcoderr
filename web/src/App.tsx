import { Routes, Route, Navigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "./api/client";
import Sidebar from "./components/sidebar";
import Dashboard from "./pages/dashboard";
import FlowsList from "./pages/flows-list";
import FlowDetail from "./pages/flow-detail";
import RunsList from "./pages/runs-list";
import RunDetail from "./pages/run-detail";
import Sources from "./pages/sources";
import Plugins from "./pages/plugins";
import Settings from "./pages/settings";
import Login from "./pages/login";

export default function App() {
  const me = useQuery({ queryKey: ["me"], queryFn: api.auth.me });
  if (me.isLoading) return null;
  if (me.data?.auth_required && !me.data?.authed) return <Login onLoggedIn={() => me.refetch()} />;
  return (
    <div style={{ display: "flex", height: "100vh" }}>
      <Sidebar />
      <main style={{ flex: 1, overflow: "auto" }}>
        <Routes>
          <Route path="/" element={<Navigate to="/dashboard" />} />
          <Route path="/dashboard" element={<Dashboard />} />
          <Route path="/flows" element={<FlowsList />} />
          <Route path="/flows/:id" element={<FlowDetail />} />
          <Route path="/runs" element={<RunsList />} />
          <Route path="/runs/:id" element={<RunDetail />} />
          <Route path="/sources" element={<Sources />} />
          <Route path="/plugins" element={<Plugins />} />
          <Route path="/settings" element={<Settings />} />
        </Routes>
      </main>
    </div>
  );
}
