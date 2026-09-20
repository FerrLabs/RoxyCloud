export type AppPassword = {
  id: string;
  name: string;
  created_at: string;
  last_used_at: string | null;
};

export type MintedPassword = AppPassword & { secret: string };

export function davUrl(): string {
  const base = ROXYCLOUD_API_URL.length > 0 ? ROXYCLOUD_API_URL : location.origin;
  return `${base.replace(/\/$/, '')}/dav`;
}

export function describeUse(password: AppPassword, now = new Date()): string {
  const made = new Date(password.created_at);
  const parts = [Number.isNaN(made.getTime()) ? 'Added' : `Added ${day(made, now)}`];
  if (password.last_used_at === null) {
    parts.push('never used');
  } else {
    const used = new Date(password.last_used_at);
    parts.push(Number.isNaN(used.getTime()) ? 'never used' : `last used ${day(used, now)}`);
  }
  return parts.join(', ');
}

function day(at: Date, now: Date): string {
  const sameYear = at.getFullYear() === now.getFullYear();
  return at.toLocaleDateString(undefined, {
    day: 'numeric',
    month: 'short',
    ...(sameYear ? {} : { year: 'numeric' }),
  });
}
