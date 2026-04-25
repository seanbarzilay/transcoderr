import { useState } from "react";
import { api } from "../api/client";

export default function Login({ onLoggedIn }: { onLoggedIn: () => void }) {
  const [pw, setPw] = useState("");
  const [err, setErr] = useState<string | null>(null);

  return (
    <div style={{ display: "flex", alignItems: "center", justifyContent: "center", height: "100vh" }}>
      <form
        onSubmit={async (e) => {
          e.preventDefault();
          setErr(null);
          try {
            await api.auth.login(pw);
            onLoggedIn();
          } catch (ex: any) {
            setErr(ex?.message ?? "login failed");
          }
        }}
        style={{ background: "rgba(255,255,255,0.06)", padding: 24, borderRadius: 8, minWidth: 320 }}
      >
        <h2 style={{ marginTop: 0 }}>transcoderr</h2>
        <input
          type="password"
          placeholder="Password"
          value={pw}
          onChange={(e) => setPw(e.target.value)}
          style={{ width: "100%", padding: 8, marginBottom: 12, fontSize: 14 }}
        />
        <button type="submit" style={{ width: "100%", padding: 10, fontSize: 14 }}>Sign in</button>
        {err && <div style={{ color: "#f88", marginTop: 8 }}>{err}</div>}
      </form>
    </div>
  );
}
