import {
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  afterNextRender,
  inject,
  input,
  output,
  resource,
  signal,
  viewChild,
} from '@angular/core';
import { describeAccess, type Access, type Given } from '../grant';
import type { Node } from '../node';
import { PLATFORM } from '../platform';
import { endOfDay, linkFor, today, type Minted } from '../share';

type Mode = 'link' | 'account';

@Component({
  selector: 'rx-share-dialog',
  templateUrl: './share-dialog.html',
  styleUrl: './share-dialog.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class ShareDialog {
  private readonly platform = inject(PLATFORM);

  readonly node = input.required<Node>();
  readonly path = input.required<string>();
  readonly dismissed = output<void>();
  readonly created = output<void>();

  private readonly dialog = viewChild.required<ElementRef<HTMLDialogElement>>('dialog');

  protected readonly canGrant = this.platform.grant !== undefined;
  protected readonly mode = signal<Mode>('link');

  protected readonly expiry = signal('');
  protected readonly earliest = today();
  protected readonly password = signal('');
  protected readonly busy = signal(false);
  protected readonly failure = signal<string | null>(null);
  protected readonly minted = signal<Minted | null>(null);
  protected readonly copied = signal(false);

  protected readonly email = signal('');
  protected readonly access = signal<Access>('read');
  protected readonly announcement = signal<string | null>(null);
  private readonly granted = signal(0);

  protected readonly grants = resource({
    params: () =>
      this.mode() === 'account' ? { node: this.node().id, version: this.granted() } : undefined,
    loader: async ({ params }) =>
      (await (this.platform.listGrants?.() ?? Promise.resolve([]))).filter(
        (given) => given.node_id === params.node,
      ),
  });

  protected readonly describe = describeAccess;

  constructor() {
    afterNextRender(() => this.dialog().nativeElement.showModal());
  }

  protected choose(mode: Mode): void {
    this.mode.set(mode);
    this.failure.set(null);
    this.announcement.set(null);
  }

  protected async mint(): Promise<void> {
    const create = this.platform.share;
    if (create === undefined) {
      return;
    }

    const day = this.expiry();
    const expires = day.length === 0 ? undefined : endOfDay(day);
    if (expires === null) {
      this.failure.set('That expiry is not a date the browser understands.');
      return;
    }

    this.failure.set(null);
    this.busy.set(true);
    try {
      const password = this.password().trim();
      this.minted.set(
        await create({
          path: this.path(),
          expires_at: expires,
          password: password.length > 0 ? password : undefined,
        }),
      );
      this.created.emit();
    } catch (cause: unknown) {
      this.failure.set(reasonFor(cause));
    } finally {
      this.busy.set(false);
    }
  }

  protected async give(): Promise<void> {
    const grant = this.platform.grant;
    const email = this.email().trim();
    if (grant === undefined || email.length === 0) {
      return;
    }

    this.failure.set(null);
    this.announcement.set(null);
    this.busy.set(true);
    try {
      const given = await grant({ path: this.path(), email, access: this.access() });
      this.email.set('');
      this.announcement.set(`Shared with ${given.email}, ${describeAccess(given.access)}`);
      this.granted.update((count) => count + 1);
      this.created.emit();
    } catch (cause: unknown) {
      this.failure.set(reasonFor(cause));
    } finally {
      this.busy.set(false);
    }
  }

  protected async take(given: Given): Promise<void> {
    const withdraw = this.platform.withdrawGrant;
    if (withdraw === undefined) {
      return;
    }

    this.failure.set(null);
    try {
      await withdraw(given.id);
      this.announcement.set(`${given.email} no longer has access`);
      this.granted.update((count) => count + 1);
      this.created.emit();
    } catch (cause: unknown) {
      this.failure.set(reasonFor(cause));
    }
  }

  protected url(): string {
    const minted = this.minted();
    return minted === null ? '' : linkFor(minted.token);
  }

  protected async copy(): Promise<void> {
    try {
      await navigator.clipboard.writeText(this.url());
      this.copied.set(true);
    } catch {
      this.copied.set(false);
      this.failure.set('Copying failed. Select the link and copy it by hand.');
    }
  }

  protected dismiss(): void {
    this.dialog().nativeElement.close();
    this.dismissed.emit();
  }
}

function reasonFor(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
