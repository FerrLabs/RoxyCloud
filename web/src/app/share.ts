import type { NodeKind } from './node';

export type Share = {
  id: string;
  node_id: string;
  name: string;
  has_password: boolean;
  expires_at: string | null;
  created_at: string;
  last_used_at: string | null;
};

export type Minted = Share & { token: string };

export type NewShare = {
  path: string;
  expires_at?: string;
  password?: string;
};

export type PublicEntry = {
  name: string;
  kind: NodeKind;
  size: number;
  updated_at: string;
};

export type Linked = {
  entry: PublicEntry;
  children: PublicEntry[];
};

export const SHARE_PREFIX = '/s';

export function linkFor(token: string): string {
  const origin = typeof location === 'undefined' ? '' : location.origin;
  return `${origin}/#${SHARE_PREFIX}/${token}`;
}

export function describeExpiry(share: Share, now = new Date()): string {
  if (share.expires_at === null) {
    return 'No expiry';
  }
  const at = new Date(share.expires_at);
  if (Number.isNaN(at.getTime())) {
    return 'No expiry';
  }
  const label = at.toLocaleDateString(undefined, {
    day: 'numeric',
    month: 'short',
    year: 'numeric',
  });
  return at.getTime() <= now.getTime() ? `Expired ${label}` : `Expires ${label}`;
}

export function describeUse(share: Share): string {
  if (share.last_used_at === null) {
    return 'Never opened';
  }
  const at = new Date(share.last_used_at);
  return Number.isNaN(at.getTime())
    ? 'Never opened'
    : `Last opened ${at.toLocaleDateString(undefined, { day: 'numeric', month: 'short' })}`;
}

export function endOfDay(day: string): string | undefined {
  if (day.length === 0) {
    return undefined;
  }
  const at = new Date(`${day}T23:59:59`);
  return Number.isNaN(at.getTime()) ? undefined : at.toISOString();
}

export function describeLink(share: Share): string {
  const parts = [describeExpiry(share), describeUse(share)];
  if (share.has_password) {
    parts.push('Password protected');
  }
  return parts.join(', ');
}
