import {
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  afterNextRender,
  input,
  output,
  viewChild,
} from '@angular/core';

@Component({
  selector: 'rx-confirm',
  templateUrl: './confirm.html',
  styleUrl: './confirm.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Confirm {
  readonly heading = input.required<string>();
  readonly body = input.required<string>();
  readonly action = input('Confirm');
  readonly accepted = output<void>();
  readonly dismissed = output<void>();

  private readonly dialog = viewChild.required<ElementRef<HTMLDialogElement>>('dialog');

  constructor() {
    afterNextRender(() => this.dialog().nativeElement.showModal());
  }

  protected accept(): void {
    this.dialog().nativeElement.close();
    this.accepted.emit();
  }

  protected dismiss(): void {
    this.dialog().nativeElement.close();
    this.dismissed.emit();
  }
}
