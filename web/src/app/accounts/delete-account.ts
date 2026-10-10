import {
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  afterNextRender,
  computed,
  input,
  output,
  signal,
  viewChild,
} from '@angular/core';
import { describeUsage, type ManagedAccount } from './managed';

@Component({
  selector: 'rx-delete-account',
  templateUrl: './delete-account.html',
  styleUrl: './account-password.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class DeleteAccountDialog {
  readonly account = input.required<ManagedAccount>();
  readonly others = input.required<ManagedAccount[]>();
  readonly submitted = output<string | null>();
  readonly dismissed = output<void>();

  private readonly dialog = viewChild.required<ElementRef<HTMLDialogElement>>('dialog');

  protected readonly recipient = signal('');
  protected readonly usage = computed(() => describeUsage(this.account()));
  protected readonly candidates = computed(() =>
    this.others().filter(
      (other) =>
        other.id !== this.account().id &&
        other.disabled_at === undefined &&
        other.role !== 'reader',
    ),
  );

  constructor() {
    afterNextRender(() => this.dialog().nativeElement.showModal());
  }

  protected submit(): void {
    this.dialog().nativeElement.close();
    this.submitted.emit(this.recipient() === '' ? null : this.recipient());
  }

  protected dismiss(): void {
    this.dialog().nativeElement.close();
    this.dismissed.emit();
  }
}
