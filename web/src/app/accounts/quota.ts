import {
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  afterNextRender,
  input,
  output,
  signal,
  viewChild,
} from '@angular/core';
import { bytesFrom, gigabytesOf, type ManagedAccount } from './managed';

@Component({
  selector: 'rx-quota',
  templateUrl: './quota.html',
  styleUrl: './account-password.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class QuotaDialog {
  readonly account = input.required<ManagedAccount>();
  readonly submitted = output<number>();
  readonly dismissed = output<void>();

  private readonly dialog = viewChild.required<ElementRef<HTMLDialogElement>>('dialog');

  protected readonly gigabytes = signal('');
  protected readonly failure = signal<string | null>(null);

  constructor() {
    afterNextRender(() => {
      this.gigabytes.set(gigabytesOf(this.account().bytes_max));
      this.dialog().nativeElement.showModal();
    });
  }

  protected submit(): void {
    const bytes = bytesFrom(this.gigabytes());
    if (bytes === null) {
      this.failure.set('A quota is a number of gigabytes above zero.');
      return;
    }
    this.dialog().nativeElement.close();
    this.submitted.emit(bytes);
  }

  protected dismiss(): void {
    this.dialog().nativeElement.close();
    this.dismissed.emit();
  }
}
