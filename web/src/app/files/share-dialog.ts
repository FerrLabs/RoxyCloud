import {
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  afterNextRender,
  inject,
  input,
  output,
  signal,
  viewChild,
} from '@angular/core';
import type { Node } from '../node';
import { PLATFORM } from '../platform';
import { endOfDay, linkFor, type Minted } from '../share';

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

  protected readonly expiry = signal('');
  protected readonly password = signal('');
  protected readonly busy = signal(false);
  protected readonly failure = signal<string | null>(null);
  protected readonly minted = signal<Minted | null>(null);
  protected readonly copied = signal(false);

  constructor() {
    afterNextRender(() => this.dialog().nativeElement.showModal());
  }

  protected async mint(): Promise<void> {
    const create = this.platform.share;
    if (create === undefined) {
      return;
    }

    this.failure.set(null);
    this.busy.set(true);
    try {
      const password = this.password().trim();
      this.minted.set(
        await create({
          path: this.path(),
          expires_at: endOfDay(this.expiry()),
          password: password.length > 0 ? password : undefined,
        }),
      );
      this.created.emit();
    } catch (cause: unknown) {
      this.failure.set(cause instanceof Error ? cause.message : String(cause));
    } finally {
      this.busy.set(false);
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
