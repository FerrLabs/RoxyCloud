import { InjectionToken, type Provider } from '@angular/core';
import type { Account } from './account';
import type { Node } from './node';

export type PlatformKind = 'browser' | 'desktop';

export interface Platform {
  readonly kind: PlatformKind;
  authenticated(): boolean;
  login(email: string, password: string): Promise<void>;
  account(): Promise<Account>;
  listFolder(path: string): Promise<Node[]>;
  read(path: string): Promise<Blob>;
  download(path: string, name: string): Promise<string | null>;
  remove(path: string): Promise<void>;
  rename(from: string, to: string): Promise<Node>;
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
      throw new Error(await messageFor(response));
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
    account: () => json<Account>('/v1/auth/me'),
    listFolder: (path) => json<Node[]>(`/v1/folders${encodePath(path)}`),
    read: async (path) => (await call(`/v1/files${encodePath(path)}`)).blob(),
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
    rename: (from, to) =>
      json<Node>('/v1/move', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ from, to }),
      }),
  };
}

async function messageFor(response: Response): Promise<string> {
  switch (response.status) {
    case 403:
      return 'this account may only read';
    case 507:
      return 'there is no room left in this account';
    default:
      return (await explanationFrom(response)) ?? `${response.status} ${response.statusText}`;
  }
}

async function explanationFrom(response: Response): Promise<string | null> {
  const body: unknown = await response.json().catch(() => null);
  const explanation =
    typeof body === 'object' && body !== null ? (body as { error?: unknown }).error : null;
  return typeof explanation === 'string' && explanation.length > 0 ? explanation : null;
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
    account: async () => {
      const { invoke } = await core();
      return invoke<Account>('account');
    },
    listFolder: async (path) => {
      const { invoke } = await core();
      return invoke<Node[]>('list_folder', { path });
    },
    read: async (path) => {
      const { invoke } = await core();
      return new Blob([await invoke<ArrayBuffer>('read_file', { path })]);
    },
    download: async (path) => {
      const { invoke } = await core();
      return invoke<string>('download_file', { path });
    },
    remove: async (path) => {
      const { invoke } = await core();
      await invoke<void>('delete_node', { path });
    },
    rename: async (from, to) => {
      const { invoke } = await core();
      return invoke<Node>('move_node', { from, to });
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
