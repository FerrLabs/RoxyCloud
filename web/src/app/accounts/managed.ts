import type { Role } from '../account';
import { formatSize } from '../node';

export type ManagedAccount = {
  id: string;
  email: string;
  display_name: string;
  role: Role;
  bytes_used?: number;
  bytes_max?: number;
  disabled_at?: string;
};

export type NewAccount = {
  email: string;
  display_name: string;
  password: string;
  role: Role;
};

export const ROLES: Role[] = ['admin', 'member', 'reader'];

const GIGABYTE = 1000 * 1000 * 1000;

export function describeUsage(account: ManagedAccount): string {
  if (account.bytes_max === undefined) {
    return 'Nothing stored yet';
  }
  const used = account.bytes_used ?? 0;
  return `${formatSize(used)} of ${formatSize(account.bytes_max)}`;
}

export function gigabytesOf(bytes: number | undefined): string {
  if (bytes === undefined) {
    return '';
  }
  const gigabytes = bytes / GIGABYTE;
  return String(Number(gigabytes.toFixed(2)));
}

export function bytesFrom(gigabytes: string): number | null {
  const value = Number(gigabytes.trim());
  if (!Number.isFinite(value) || value <= 0) {
    return null;
  }
  return Math.round(value * GIGABYTE);
}

export function generatePassword(): string {
  const alphabet = 'abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789';
  const bytes = new Uint32Array(20);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (value) => alphabet[value % alphabet.length]).join('');
}
