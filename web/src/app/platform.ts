import { InjectionToken, type Provider } from '@angular/core';
import type { Account, Role } from './account';
import type { AppPassword, MintedPassword } from './account/app-password';
import type { ManagedAccount, NewAccount } from './accounts/managed';
import type { Given, NewGrant, Received } from './grant';
import type { Hit, Node, Trashed } from './node';
import type { Dropping, Outgoing } from './outgoing';
import {
  encodePath,
  saveStreaming,
  upload as uploadChunked,
  type Link,
  type Progress,
} from './transfer';
import type { Minted, NewShare, Share } from './share';
import type { SyncCommand, Syncing } from './sync/syncing';
import type { Version } from './version';

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
  resume?(): Promise<boolean>;
  login(email: string, password: string, server?: string): Promise<string | null>;
  server?(): string;
  checkUpdate?(): Promise<Release>;
  installUpdate?(): Promise<void>;
  pickFolder?(): Promise<string | null>;
  startSync?(folder: string): Promise<void>;
  controlSync?(command: SyncCommand): Promise<void>;
  syncStatus?(): Promise<Syncing | null>;
  watchSync?(listener: (syncing: Syncing) => void): Promise<() => void>;
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
  deleteAccount?(id: string, handOverTo: string | null): Promise<void>;
  account(): Promise<Account>;
  listFolder(path: string): Promise<Node[]>;
  read(path: string): Promise<Blob>;
  download(path: string, name: string): Promise<string | null>;
  remove(path: string): Promise<void>;
  rename(from: string, to: string): Promise<Node>;
  createFolder?(path: string): Promise<Node>;
  search?(query: string): Promise<Hit[]>;
  upload?(path: string, file: File, progress?: Progress): Promise<void>;
  pickUploads?(): Promise<Outgoing[]>;
  watchDrops?(listener: (dropping: Dropping) => void): Promise<() => void>;
  listShares?(): Promise<Share[]>;
  share?(request: NewShare): Promise<Minted>;
  revokeShare?(id: string): Promise<void>;
  listGrants?(): Promise<Given[]>;
  grant?(request: NewGrant): Promise<Given>;
  receivedGrants?(): Promise<Received[]>;
  withdrawGrant?(id: string): Promise<void>;
  listVersions?(path: string): Promise<Version[]>;
  downloadVersion?(path: string, id: string, name: string): Promise<string | null>;
  restoreVersion?(path: string, id: string): Promise<Node>;
  listTrash?(): Promise<Trashed[]>;
  restoreFromTrash?(id: string): Promise<Node>;
  purgeFromTrash?(id: string): Promise<void>;
  emptyTrash?(): Promise<void>;
}

export const PLATFORM = new InjectionToken<Platform>('Stashden platform');

export class RequestFailed extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

const TOKEN_KEY = 'stashden.token';
const SERVER_KEY = 'stashden.server';
const KEY_PREFIX = 'stashden.';
const LEGACY_KEY_PREFIX = 'roxycloud.';

function adoptLegacyKeys(): void {
  try {
    const legacyKeys = Array.from({ length: localStorage.length }, (_, index) =>
      localStorage.key(index),
    ).filter((key): key is string => key?.startsWith(LEGACY_KEY_PREFIX) ?? false);
    for (const legacy of legacyKeys) {
      const current = KEY_PREFIX + legacy.slice(LEGACY_KEY_PREFIX.length);
      const value = localStorage.getItem(legacy);
      if (value !== null && localStorage.getItem(current) === null) {
        localStorage.setItem(current, value);
      }
      localStorage.removeItem(legacy);
    }
  } catch {
    return;
  }
}

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

  const link: Link = {
    base: baseUrl,
    headers: bearer,
    fail: async (status, text) =>
      new RequestFailed(status, await messageFor(new Response(text, { status }))),
  };

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
      return null;
    },
    signOut: async () => {
      try {
        await call('/v1/auth/logout', { method: 'POST' });
      } catch (cause) {
        if (!(cause instanceof RequestFailed && cause.status === 401)) {
          throw cause;
        }
      } finally {
        localStorage.removeItem(TOKEN_KEY);
      }
    },
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
    deleteAccount: async (id, handOverTo) => {
      const query = handOverTo === null ? '' : `?hand_over_to=${encodeURIComponent(handOverTo)}`;
      await call(`/v1/users/${encodeURIComponent(id)}${query}`, { method: 'DELETE' });
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
    upload: (path, file, progress) => uploadChunked(link, path, file, progress),
    download: async (path, name) => {
      await saveStreaming(() => call(`/v1/files${encodePath(path)}`), name);
      return null;
    },
    remove: async (path) => {
      await call(`/v1/files${encodePath(path)}`, { method: 'DELETE' });
    },
    createFolder: (path) => json<Node>(`/v1/folders${encodePath(path)}`, { method: 'POST' }),
    search: (query) => json<Hit[]>(`/v1/search?q=${encodeURIComponent(query)}`),
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
    listVersions: (path) => json<Version[]>(`/v1/versions${encodePath(path)}`),
    downloadVersion: async (path, id, name) => {
      const address = `/v1/version/${encodeURIComponent(id)}${encodePath(path)}`;
      await saveStreaming(() => call(address), name);
      return null;
    },
    restoreVersion: (path, id) =>
      json<Node>(`/v1/version/${encodeURIComponent(id)}${encodePath(path)}`, { method: 'POST' }),
    listTrash: () => json<Trashed[]>('/v1/trash'),
    restoreFromTrash: (id) =>
      json<Node>(`/v1/trash/${encodeURIComponent(id)}/restore`, { method: 'POST' }),
    purgeFromTrash: async (id) => {
      await call(`/v1/trash/${encodeURIComponent(id)}`, { method: 'DELETE' });
    },
    emptyTrash: async () => {
      await call('/v1/trash', { method: 'DELETE' });
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
    resume: async () => {
      const { invoke } = await core();
      connected = await invoke<boolean>('resume');
      return connected;
    },
    server: remembered,
    login: async (email, password, server) => {
      const address = addressOf(server ?? remembered());
      const { invoke } = await core();
      const warning = await invoke<string | null>('login', { server: address, email, password });
      try {
        localStorage.setItem(SERVER_KEY, address);
      } catch {
        // A browser that refuses storage still signs in, it just forgets the address.
      }
      connected = true;
      return warning;
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
    pickFolder: async () => {
      const { invoke } = await core();
      return invoke<string | null>('pick_folder');
    },
    startSync: async (folder) => {
      const { invoke } = await core();
      await invoke<void>('start_sync', { folder });
    },
    controlSync: async (command) => {
      const { invoke } = await core();
      await invoke<void>('sync_control', { command });
    },
    syncStatus: async () => {
      const { invoke } = await core();
      return invoke<Syncing | null>('sync_status');
    },
    watchSync: async (listener) => {
      const { listen } = await import('@tauri-apps/api/event');
      return listen<Syncing>('sync:status', (event) => listener(event.payload));
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
    createFolder: (path) => command<Node>('create_folder', { path }),
    search: (query) => command<Hit[]>('search_nodes', { query }),
    listTrash: () => command<Trashed[]>('list_trash'),
    restoreFromTrash: (id) => command<Node>('restore_from_trash', { id }),
    purgeFromTrash: (id) => command<void>('purge_from_trash', { id }),
    emptyTrash: () => command<void>('empty_trash'),
    listVersions: (path) => command<Version[]>('list_versions', { path }),
    downloadVersion: (path, id) => command<string>('download_version', { path, id }),
    restoreVersion: (path, id) => command<Node>('restore_version', { path, id }),
    listShares: () => command<Share[]>('list_shares'),
    share: (request) => command<Minted>('share', { request }),
    revokeShare: (id) => command<void>('revoke_share', { id }),
    listGrants: () => command<Given[]>('list_grants'),
    grant: (request) => command<Given>('grant', { request }),
    receivedGrants: () => command<Received[]>('received_grants'),
    withdrawGrant: (id) => command<void>('withdraw_grant', { id }),
    pickUploads: async () => fromPicked(await command<Picked[]>('pick_uploads')),
    watchDrops: async (listener) => {
      const { getCurrentWebview } = await import('@tauri-apps/api/webview');
      return getCurrentWebview().onDragDropEvent(({ payload }) => {
        switch (payload.type) {
          case 'enter':
          case 'over':
            listener({ kind: 'over' });
            break;
          case 'leave':
            listener({ kind: 'leave' });
            break;
          case 'drop':
            void command<Picked[]>('describe_drops', { paths: payload.paths }).then((picked) =>
              listener({ kind: 'drop', items: fromPicked(picked) }),
            );
            break;
        }
      });
    },
  };
}

async function command<T>(name: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import('@tauri-apps/api/core');
  try {
    return await invoke<T>(name, args);
  } catch (cause: unknown) {
    throw failureFrom(cause);
  }
}

function failureFrom(cause: unknown): Error {
  if (typeof cause !== 'object' || cause === null || !('message' in cause)) {
    return new Error(String(cause));
  }
  const { status, message } = cause as { status?: unknown; message: unknown };
  return typeof status === 'number'
    ? new RequestFailed(status, String(message))
    : new Error(String(message));
}

type Picked = { name: string; source: string };

function fromPicked(picked: Picked[]): Outgoing[] {
  return picked.map(({ name, source }) => ({
    name,
    send: async (destination) => {
      await command<Node>('upload_file', { path: destination, source });
    },
  }));
}

function addressOf(server: string): string {
  const trimmed = server.trim().replace(/\/+$/, '');
  return /^https?:\/\//.test(trimmed) ? trimmed : `https://${trimmed}`;
}

export { encodePath };

export function resolvePlatform(baseUrl: string): Platform {
  adoptLegacyKeys();
  return isDesktop() ? desktopPlatform(baseUrl) : browserPlatform(baseUrl);
}

export function providePlatform(): Provider {
  return { provide: PLATFORM, useFactory: () => resolvePlatform(STASHDEN_API_URL) };
}
