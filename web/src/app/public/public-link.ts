import {
  ChangeDetectionStrategy,
  Component,
  computed,
  inject,
  resource,
  signal,
} from '@angular/core';
import { toSignal } from '@angular/core/rxjs-interop';
import { ActivatedRoute } from '@angular/router';
import { formatDate, formatSize } from '../node';
import type { PublicEntry } from '../share';
import { LinkNeedsPassword, PublicLinks } from './links';

@Component({
  selector: 'rx-public-link',
  templateUrl: './public-link.html',
  styleUrl: './public-link.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class PublicLink {
  private readonly links = inject(PublicLinks);

  private readonly params = toSignal(inject(ActivatedRoute).params, { initialValue: {} });

  protected readonly token = computed(() => String((this.params() as { token?: string }).token));
  protected readonly password = signal('');
  protected readonly draft = signal('');
  protected readonly trail = signal<string[]>([]);
  protected readonly failure = signal<string | null>(null);
  protected readonly busy = signal(false);

  protected readonly path = computed(() => this.trail().join('/'));

  protected readonly link = resource({
    params: () => ({ token: this.token(), password: this.password(), path: this.path() }),
    loader: ({ params }) => this.links.open(params.token, params.password, params.path),
  });

  protected readonly needsPassword = computed(
    () => this.link.error() instanceof LinkNeedsPassword,
  );

  protected readonly entry = computed(() => this.link.value()?.entry ?? null);
  protected readonly children = computed(() => this.link.value()?.children ?? []);

  protected readonly size = formatSize;
  protected readonly date = formatDate;

  protected unlock(): void {
    this.password.set(this.draft());
  }

  protected enter(child: PublicEntry): void {
    if (child.kind === 'directory') {
      this.trail.update((trail) => [...trail, child.name]);
    }
  }

  protected upTo(depth: number): void {
    this.trail.update((trail) => trail.slice(0, depth));
  }

  protected async save(child: PublicEntry | null): Promise<void> {
    const target = child === null ? this.entry() : child;
    if (target === null) {
      return;
    }
    const below = child === null ? this.path() : [...this.trail(), child.name].join('/');

    this.failure.set(null);
    this.busy.set(true);
    try {
      await this.links.download(this.token(), this.password(), below, target.name);
    } catch (cause: unknown) {
      this.failure.set(cause instanceof Error ? cause.message : String(cause));
    } finally {
      this.busy.set(false);
    }
  }
}
