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
import { PLATFORM } from '../platform';

@Component({
  selector: 'rx-change-password',
  templateUrl: './change-password.html',
  styleUrl: './change-password.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class ChangePassword {
  private readonly platform = inject(PLATFORM);

  readonly dismissed = output<void>();
  readonly changed = output<void>();

  private readonly dialog = viewChild.required<ElementRef<HTMLDialogElement>>('dialog');

  protected readonly current = signal('');
  protected readonly replacement = signal('');
  protected readonly busy = signal(false);
  protected readonly failure = signal<string | null>(null);

  constructor() {
    afterNextRender(() => this.dialog().nativeElement.showModal());
  }

  protected async submit(): Promise<void> {
    const change = this.platform.changePassword;
    if (change === undefined || this.busy()) {
      return;
    }

    this.failure.set(null);
    this.busy.set(true);
    try {
      await change(this.current(), this.replacement());
      this.dialog().nativeElement.close();
      this.changed.emit();
    } catch (cause: unknown) {
      this.failure.set(cause instanceof Error ? cause.message : String(cause));
    } finally {
      this.busy.set(false);
    }
  }

  protected dismiss(): void {
    this.dialog().nativeElement.close();
    this.dismissed.emit();
  }
}
