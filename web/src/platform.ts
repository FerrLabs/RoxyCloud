import type { Node } from "./types";

export type Platform = {
  kind: "browser" | "desktop";
  login(email: string, password: string): Promise<void>;
  listFolder(path: string): Promise<Node[]>;
};

const isDesktop = () =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

function browserPlatform(
  baseUrl: string,
  token: () => string | null,
  storeToken: (value: string) => void,
): Platform {
  const request = async <T>(path: string, init?: RequestInit): Promise<T> => {
    const bearer = token();
    const response = await fetch(`${baseUrl}${path}`, {
      ...init,
      headers: {
        ...(init?.body ? { "Content-Type": "application/json" } : {}),
        ...(bearer ? { Authorization: `Bearer ${bearer}` } : {}),
      },
    });
    if (!response.ok) {
      throw new Error(`${response.status} ${response.statusText}`);
    }
    return (await response.json()) as T;
  };

  return {
    kind: "browser",
    login: async (email, password) => {
      const session = await request<{ token: string }>("/v1/auth/login", {
        method: "POST",
        body: JSON.stringify({ email, password }),
      });
      storeToken(session.token);
    },
    listFolder: (path) => request<Node[]>(`/v1/folders${encodePath(path)}`),
  };
}

async function desktopPlatform(serverUrl: string): Promise<Platform> {
  const { invoke } = await import("@tauri-apps/api/core");
  return {
    kind: "desktop",
    login: (email, password) =>
      invoke<void>("login", { server: serverUrl, email, password }),
    listFolder: (path) => invoke<Node[]>("list_folder", { path }),
  };
}

export function encodePath(path: string): string {
  const segments = path.split("/").filter((segment) => segment.length > 0);
  if (segments.length === 0) {
    return "";
  }
  return `/${segments.map(encodeURIComponent).join("/")}`;
}

export function resolvePlatform(
  baseUrl: string,
  token: () => string | null,
  storeToken: (value: string) => void,
): Promise<Platform> {
  return isDesktop()
    ? desktopPlatform(baseUrl)
    : Promise.resolve(browserPlatform(baseUrl, token, storeToken));
}
