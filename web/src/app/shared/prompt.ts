import {
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  afterNextRender,
  input,
  linkedSignal,
  output,
  viewChild,
} from '@angular/core';

@Component({
  selector: 'rx-prompt',
  templateUrl: './prompt.html',
  styleUrl: './prompt.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Prompt {
  readonly heading = input.required<string>();
  readonly label = input.required<string>();
  readonly hint = input('');
  readonly value = input('');
  readonly action = input('Save');
  readonly submitted = output<string>();
  readonly dismissed = output<void>();

  private readonly dialog = viewChild.required<ElementRef<HTMLDialogElement>>('dialog');
  private readonly field = viewChild.required<ElementRef<HTMLInputElement>>('field');

  protected readonly draft = linkedSignal(() => this.value());

  constructor() {
    afterNextRender(() => {
      this.dialog().nativeElement.showModal();
      const field = this.field().nativeElement;
      field.focus();
      field.setSelectionRange(0, stemLength(this.value()));
    });
  }

  protected submit(): void {
    const value = this.draft().trim();
    if (value.length === 0 || value === this.value()) {
      this.dismiss();
      return;
    }
    this.dialog().nativeElement.close();
    this.submitted.emit(value);
  }

  protected dismiss(): void {
    this.dialog().nativeElement.close();
    this.dismissed.emit();
  }
}

function stemLength(name: string): number {
  const dot = name.lastIndexOf('.');
  return dot > 0 ? dot : name.length;
}
