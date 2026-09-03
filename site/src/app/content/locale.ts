export const LOCALES = ['en', 'fr'] as const;

export type Locale = (typeof LOCALES)[number];

export const DEFAULT_LOCALE: Locale = 'en';

export function localeOf(url: string): Locale {
  return url === '/fr' || url.startsWith('/fr/') ? 'fr' : DEFAULT_LOCALE;
}

export function withoutLocale(url: string): string {
  const path = localeOf(url) === 'fr' ? url.slice('/fr'.length) : url;
  return path.startsWith('/') ? path : `/${path}`;
}

export function withLocale(locale: Locale, path: string): string {
  const normalised = path === '/' ? '' : path;
  return locale === DEFAULT_LOCALE ? normalised || '/' : `/fr${normalised}`;
}
