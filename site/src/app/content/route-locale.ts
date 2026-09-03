import type { ActivatedRoute } from '@angular/router';
import { DEFAULT_LOCALE, LOCALES, type Locale } from './locale';

export function routeLocale(route: ActivatedRoute): Locale {
  const declared: unknown = route.snapshot.data['locale'];
  return LOCALES.find((locale) => locale === declared) ?? DEFAULT_LOCALE;
}
