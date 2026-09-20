import {
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  afterNextRender,
  inject,
  output,
  signal,
  viewChild,
} from '@angular/core';
import { PLATFORM, type Available } from '../platform';

@Component({
  selector: 'rx-update',
  templateUrl: './update.html',
  styleUrl: './update.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Update {
  private readonly platform = inject(PLATFORM);

  readonly dismissed = output<void>();

  private readonly dialog = viewChild.required<ElementRef<HTMLDialogElement>>('dialog');

  protected readonly looking = signal(true);
  protected readonly installing = signal(false);
  protected readonly release = signal<Available | null>(null);
  protected readonly current = signal('');
  protected readonly failure = signal<string | null>(null);

  constructor() {
    afterNextRender(() => {
      this.dialog().nativeElement.showModal();
      void this.look();
    });
  }

  private async look(): Promise<void> {
    try {
      const update = await this.platform.checkUpdate?.();
      this.current.set(update?.current ?? '');
      this.release.set(update?.available ?? null);
    } catch (cause: unknown) {
      this.failure.set(cause instanceof Error ? cause.message : String(cause));
    } finally {
      this.looking.set(false);
    }
  }

  protected async install(): Promise<void> {
    this.failure.set(null);
    this.installing.set(true);
    try {
      // The app restarts into the new version, so nothing here runs on success.
      await this.platform.installUpdate?.();
    } catch (cause: unknown) {
      this.failure.set(cause instanceof Error ? cause.message : String(cause));
      this.installing.set(false);
    }
  }

  protected dismiss(): void {
    if (this.installing()) {
      return;
    }
    this.dialog().nativeElement.close();
    this.dismissed.emit();
  }
}
