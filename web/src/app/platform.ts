import { InjectionToken, type Provider } from '@angular/core';
import type { Account, Role } from './account';
import type { AppPassword, MintedPassword } from './account/app-password';
import type { ManagedAccount, NewAccount } from './accounts/managed';
import type { Given, NewGrant, Received } from './grant';
import type { Node } from './node';
import type { Minted, NewShare, Share } from './share';

export type PlatformKind = 'browser' | 'desktop';

export type Available = {
  version: string;
  notes?: string;
};

export type Release = {
  current: string;
  available: Available | null;
};

export interface Platform {
  readonly kind: PlatformKind;
  authenticated(): boolean;
  login(email: string, password: string, server?: string): Promise<void>;
  server?(): string;
  checkUpdate?(): Promise<Release>;
  installUpdate?(): Promise<void>;
  signOut(): void | Promise<void>;
  changePassword?(current: string, password: string): Promise<void>;
  listAppPasswords?(): Promise<AppPassword[]>;
  mintAppPassword?(name: string): Promise<MintedPassword>;
  revokeAppPassword?(id: string): Promise<void>;
  listAccounts?(): Promise<ManagedAccount[]>;
  createAccount?(account: NewAccount): Promise<void>;
  setRole?(id: string, role: Role): Promise<void>;
  setQuota?(id: string, bytes: number): Promise<void>;
  setDisabled?(id: string, disabled: boolean): Promise<void>;
  unlockAccount?(id: string): Promise<void>;
  resetPassword?(id: string, password: string): Promise<void>;
  account(): Promise<Account>;
  listFolder(path: string): Promise<Node[]>;
  read(path: string): Promise<Blob>;
  download(path: string, name: string): Promise<string | null>;
  remove(path: string): Promise<void>;
  rename(from: string, to: string): Promise<Node>;
  upload?(path: string, file: File): Promise<void>;
  listShares?(): Promise<Share[]>;
  share?(request: NewShare): Promise<Minted>;
  revokeShare?(id: string): Promise<void>;
  listGrants?(): Promise<Given[]>;
  grant?(request: NewGrant): Promise<Given>;
  receivedGrants?(): Promise<Received[]>;
  withdrawGrant?(id: string): Promise<void>;
}

export const PLATFORM = new InjectionToken<Platform>('RoxyCloud platform');

export class RequestFailed extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

const TOKEN_KEY = 'roxycloud.token';
const SERVER_KEY = 'roxycloud.server';

const isDesktop = () => typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

function browserPlatform(baseUrl: string): Platform {
  const bearer = (): Record<string, string> => {
    const token = localStorage.getItem(TOKEN_KEY);
    return token ? { Authorization: `Bearer ${token}` } : {};
  };

  const call = async (path: string, init?: RequestInit): Promise<Response> => {
    const response = await fetch(`${baseUrl}${path}`, {
      ...init,
      headers: {
        ...init?.headers,
        ...bearer(),
      },
    });
    if (!response.ok) {
      throw new RequestFailed(response.status, await messageFor(response));
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
    signOut: () => localStorage.removeItem(TOKEN_KEY),
    changePassword: async (current, password) => {
      const response = await fetch(`${baseUrl}/v1/auth/password`, {
        method: 'PUT',
        headers: {
          'Content-Type': 'application/json',
          ...bearer(),
        },
        body: JSON.stringify({ current, password }),
      });
      if (response.status === 401) {
        throw new Error('that is not your current password');
      }
      if (!response.ok) {
        throw new Error(await messageFor(response));
      }
    },
    listAppPasswords: () => json<AppPassword[]>('/v1/app-passwords'),
    mintAppPassword: (name) =>
      json<MintedPassword>('/v1/app-passwords', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name }),
      }),
    revokeAppPassword: async (id) => {
      await call(`/v1/app-passwords/${encodeURIComponent(id)}`, { method: 'DELETE' });
    },
    listAccounts: () => json<ManagedAccount[]>('/v1/users'),
    createAccount: async (account) => {
      await call('/v1/users', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(account),
      });
    },
    setRole: async (id, role) => {
      await call(`/v1/users/${encodeURIComponent(id)}/role`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ role }),
      });
    },
    setQuota: async (id, bytes) => {
      await call(`/v1/users/${encodeURIComponent(id)}/quota`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ bytes_max: bytes }),
      });
    },
    setDisabled: async (id, disabled) => {
      await call(`/v1/users/${encodeURIComponent(id)}/${disabled ? 'disable' : 'enable'}`, {
        method: 'POST',
      });
    },
    unlockAccount: async (id) => {
      await call(`/v1/users/${encodeURIComponent(id)}/unlock`, { method: 'POST' });
    },
    resetPassword: async (id, password) => {
      await call(`/v1/users/${encodeURIComponent(id)}/password`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ password }),
      });
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
    listShares: () => json<Share[]>('/v1/shares'),
    share: (request) =>
      json<Minted>('/v1/shares', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(request),
      }),
    revokeShare: async (id) => {
      await call(`/v1/shares/${encodeURIComponent(id)}`, { method: 'DELETE' });
    },
    listGrants: () => json<Given[]>('/v1/grants'),
    grant: (request) =>
      json<Given>('/v1/grants', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(request),
      }),
    receivedGrants: () => json<Received[]>('/v1/grants/received'),
    withdrawGrant: async (id) => {
      await call(`/v1/grants/${encodeURIComponent(id)}`, { method: 'DELETE' });
    },
  };
}

async function messageFor(response: Response): Promise<string> {
  switch (response.status) {
    case 403:
      return (await explanationFrom(response)) ?? 'this account may only read';
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

function desktopPlatform(fallback: string): Platform {
  const core = () => import('@tauri-apps/api/core');
  let connected = false;

  const remembered = (): string => {
    try {
      return localStorage.getItem(SERVER_KEY) ?? fallback;
    } catch {
      return fallback;
    }
  };

  return {
    kind: 'desktop',
    authenticated: () => connected,
    server: remembered,
    login: async (email, password, server) => {
      const address = addressOf(server ?? remembered());
      const { invoke } = await core();
      await invoke<void>('login', { server: address, email, password });
      try {
        localStorage.setItem(SERVER_KEY, address);
      } catch {
        // A browser that refuses storage still signs in, it just forgets the address.
      }
      connected = true;
    },
    signOut: async () => {
      const { invoke } = await core();
      await invoke<void>('sign_out');
      connected = false;
    },
    checkUpdate: async () => {
      const { invoke } = await core();
      return invoke<Release>('check_update');
    },
    installUpdate: async () => {
      const { invoke } = await core();
      await invoke<void>('install_update');
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

function addressOf(server: string): string {
  const trimmed = server.trim().replace(/\/+$/, '');
  return /^https?:\/\//.test(trimmed) ? trimmed : `https://${trimmed}`;
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
