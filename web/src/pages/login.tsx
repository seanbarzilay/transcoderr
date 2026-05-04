import { useState } from "react";
import { api } from "../api/client";
import { errorMessage } from "../lib/errors";

export default function Login({ onLoggedIn }: { onLoggedIn: () => void }) {
  const [pw, setPw] = useState("");
  const [err, setErr] = useState<string | null>(null);

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        height: "100vh",
      }}
    >
      <form
        onSubmit={async (e) => {
          e.preventDefault();
          setErr(null);
          try {
            await api.auth.login(pw);
            onLoggedIn();
          } catch (ex: unknown) {
            setErr(errorMessage(ex, "login failed"));
          }
        }}
        className="surface"
        style={{ padding: 28, minWidth: 340 }}
      >
        <div className="brand" style={{ marginBottom: 18 }}>
          <span className="brand-dot" />
          <span>
            transcoder<span className="brand-x">/r</span>
          </span>
        </div>
        <div className="label" style={{ marginBottom: 6 }}>
          Password
        </div>
        <input
          type="password"
          autoFocus
          value={pw}
          onChange={(e) => setPw(e.target.value)}
          style={{ width: "100%", marginBottom: 14 }}
        />
        <button type="submit" style={{ width: "100%" }}>
          Sign in
        </button>
        {err && (
          <div style={{ color: "var(--bad)", marginTop: 10, fontSize: 11 }}>
            {err}
          </div>
        )}
      </form>
    </div>
  );
}
