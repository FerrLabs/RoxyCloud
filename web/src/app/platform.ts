import { InjectionToken, type Provider } from '@angular/core';
import type { Node } from './node';

export type PlatformKind = 'browser' | 'desktop';

export interface Platform {
  readonly kind: PlatformKind;
  authenticated(): boolean;
  login(email: string, password: string): Promise<void>;
  listFolder(path: string): Promise<Node[]>;
}

export const PLATFORM = new InjectionToken<Platform>('RoxyCloud platform');

const TOKEN_KEY = 'roxycloud.token';

const isDesktop = () => typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

function browserPlatform(baseUrl: string): Platform {
  const request = async <T>(path: string, init?: RequestInit): Promise<T> => {
    const bearer = localStorage.getItem(TOKEN_KEY);
    const response = await fetch(`${baseUrl}${path}`, {
      ...init,
      headers: {
        ...(init?.body ? { 'Content-Type': 'application/json' } : {}),
        ...(bearer ? { Authorization: `Bearer ${bearer}` } : {}),
      },
    });
    if (!response.ok) {
      throw new Error(`${response.status} ${response.statusText}`);
    }
    return (await response.json()) as T;
  };

  return {
    kind: 'browser',
    authenticated: () => localStorage.getItem(TOKEN_KEY) !== null,
    login: async (email, password) => {
      const session = await request<{ token: string }>('/v1/auth/login', {
        method: 'POST',
        body: JSON.stringify({ email, password }),
      });
      localStorage.setItem(TOKEN_KEY, session.token);
    },
    listFolder: (path) => request<Node[]>(`/v1/folders${encodePath(path)}`),
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
