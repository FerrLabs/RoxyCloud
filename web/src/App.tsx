import { useEffect, useState, type FormEvent } from "react";
import { resolvePlatform, type Platform } from "./platform";
import type { Node } from "./types";

const API_URL = import.meta.env["VITE_API_URL"] ?? "http://localhost:3001";
const SOURCE_URL =
  import.meta.env["VITE_SOURCE_URL"] ?? "https://github.com/FerrLabs/RoxyCloud";
const TOKEN_KEY = "roxycloud.token";

export default function App() {
  const [platform, setPlatform] = useState<Platform | null>(null);
  const [nodes, setNodes] = useState<Node[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    resolvePlatform(
      API_URL,
      () => localStorage.getItem(TOKEN_KEY),
      (value) => localStorage.setItem(TOKEN_KEY, value),
    )
      .then(setPlatform)
      .catch((cause: unknown) => setError(String(cause)));
  }, []);

  const browse = (host: Platform) =>
    host
      .listFolder("/")
      .then(setNodes)
      .catch((cause: unknown) => setError(String(cause)));

  useEffect(() => {
    if (!platform) return;
    if (platform.kind === "browser" && !localStorage.getItem(TOKEN_KEY)) return;
    void browse(platform);
  }, [platform]);

  if (!platform) return <p>Loading</p>;

  return (
    <main>
      <h1>RoxyCloud</h1>
      {error ? <p role="alert">{error}</p> : null}
      {nodes ? (
        <ul>
          {nodes.map((node) => (
            <li key={node.id}>
              {node.name}
              {node.kind === "directory" ? "/" : ` (${node.size} bytes)`}
            </li>
          ))}
        </ul>
      ) : (
        <LoginForm
          onSubmit={async (email, password) => {
            setError(null);
            try {
              await platform.login(email, password);
              await browse(platform);
            } catch (cause: unknown) {
              setError(String(cause));
            }
          }}
        />
      )}
      <footer>
        <a href={SOURCE_URL} rel="noreferrer">
          Source
        </a>
      </footer>
    </main>
  );
}

function LoginForm({
  onSubmit,
}: {
  onSubmit: (email: string, password: string) => Promise<void>;
}) {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    await onSubmit(email, password);
    setBusy(false);
  };

  return (
    <form onSubmit={(event) => void submit(event)}>
      <label>
        Email
        <input
          type="email"
          value={email}
          autoComplete="username"
          onChange={(event) => setEmail(event.target.value)}
          required
        />
      </label>
      <label>
        Password
        <input
          type="password"
          value={password}
          autoComplete="current-password"
          onChange={(event) => setPassword(event.target.value)}
          required
        />
      </label>
      <button type="submit" disabled={busy}>
        {busy ? "Signing in" : "Sign in"}
      </button>
    </form>
  );
}
