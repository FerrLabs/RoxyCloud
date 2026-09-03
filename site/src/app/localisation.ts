import { Location } from '@angular/common';
import { DOCUMENT, Injectable, computed, effect, inject } from '@angular/core';
import { toSignal } from '@angular/core/rxjs-interop';
import { Meta, Title } from '@angular/platform-browser';
import { NavigationEnd, Router } from '@angular/router';
import { filter, map } from 'rxjs';
import { CONTENT } from './content/content';
import { type Locale, localeOf, withLocale, withoutLocale } from './content/locale';

@Injectable({ providedIn: 'root' })
export class Localisation {
  private readonly router = inject(Router);
  private readonly location = inject(Location);
  private readonly document = inject(DOCUMENT);
  private readonly title = inject(Title);
  private readonly meta = inject(Meta);

  private readonly url = toSignal(
    this.router.events.pipe(
      filter((event): event is NavigationEnd => event instanceof NavigationEnd),
      map((event) => event.urlAfterRedirects),
    ),
    { initialValue: this.location.path() || '/' },
  );

  readonly locale = computed(() => localeOf(this.url()));
  readonly content = computed(() => CONTENT[this.locale()]);
  readonly alternateUrl = computed(() =>
    withLocale(this.alternate(), withoutLocale(this.url())),
  );

  constructor() {
    this.document.documentElement.lang = this.locale();
    effect(() => {
      this.document.documentElement.lang = this.locale();
    });
  }

  link(path: string): string {
    return withLocale(this.locale(), path);
  }

  describe(documentTitle: string, description: string): void {
    this.title.setTitle(documentTitle);
    this.meta.updateTag({ name: 'description', content: description });
  }

  private alternate(): Locale {
    return this.locale() === 'fr' ? 'en' : 'fr';
  }
}
