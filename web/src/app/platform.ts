import { InjectionToken, type Provider } from '@angular/core';
import type { Node } from './node';

export type PlatformKind = 'browser' | 'desktop';

export interface Platform {
  readonly kind: PlatformKind;
  authenticated(): boolean;
  login(email: string, password: string): Promise<void>;
  listFolder(path: string): Promise<Node[]>;
  download(path: string, name: string): Promise<string | null>;
  remove(path: string): Promise<void>;
  upload?(path: string, file: File): Promise<void>;
}

export const PLATFORM = new InjectionToken<Platform>('RoxyCloud platform');

const TOKEN_KEY = 'roxycloud.token';

const isDesktop = () => typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

function browserPlatform(baseUrl: string): Platform {
  const call = async (path: string, init?: RequestInit): Promise<Response> => {
    const bearer = localStorage.getItem(TOKEN_KEY);
    const response = await fetch(`${baseUrl}${path}`, {
      ...init,
      headers: {
        ...init?.headers,
        ...(bearer ? { Authorization: `Bearer ${bearer}` } : {}),
      },
    });
    if (!response.ok) {
      throw new Error(`${response.status} ${response.statusText}`);
    }
    return response;
  };

  const json = async <T>(path: string, init?: RequestInit): Promise<T> =>
    (await (await call(path, init)).json()) as T;

  return {
    kind: 'browser',
    authenticated: () => localStorage.getItem(TOKEN_KEY) !== null,
    login: async (email, password) => {
      const session = await json<{ token: string }>('/v1/auth/login', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email, password }),
      });
      localStorage.setItem(TOKEN_KEY, session.token);
    },
    listFolder: (path) => json<Node[]>(`/v1/folders${encodePath(path)}`),
    upload: async (path, file) => {
      await call(`/v1/files${encodePath(path)}`, { method: 'PUT', body: file });
    },
    download: async (path, name) => {
      const blob = await (await call(`/v1/files${encodePath(path)}`)).blob();
      const href = URL.createObjectURL(blob);
      const link = document.createElement('a');
      link.href = href;
      link.download = name;
      link.click();
      URL.revokeObjectURL(href);
      return null;
    },
    remove: async (path) => {
      await call(`/v1/files${encodePath(path)}`, { method: 'DELETE' });
    },
  };
}

function desktopPlatform(serverUrl: string): Platform {
  const core = () => import('@tauri-apps/api/core');
  let connected = false;

  return {
    kind: 'desktop',
    authenticated: () => connected,
    login: async (email, password) => {
      const { invoke } = await core();
      await invoke<void>('login', { server: serverUrl, email, password });
      connected = true;
    },
    listFolder: async (path) => {
      const { invoke } = await core();
      return invoke<Node[]>('list_folder', { path });
    },
    download: async (path) => {
      const { invoke } = await core();
      return invoke<string>('download_file', { path });
    },
    remove: async (path) => {
      const { invoke } = await core();
      await invoke<void>('delete_node', { path });
    },
  };
}

export function encodePath(path: string): string {
  const segments = path.split('/').filter((segment) => segment.length > 0);
  if (segments.length === 0) {
    return '';
  }
  return `/${segments.map(encodeURIComponent).join('/')}`;
}

export function resolvePlatform(baseUrl: string): Platform {
  return isDesktop() ? desktopPlatform(baseUrl) : browserPlatform(baseUrl);
}

export function providePlatform(): Provider {
  return { provide: PLATFORM, useFactory: () => resolvePlatform(ROXYCLOUD_API_URL) };
}
