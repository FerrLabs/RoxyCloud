import { ChangeDetectionStrategy, Component, ElementRef, input, output, viewChild } from '@angular/core';

@Component({
  selector: 'rx-upload-target',
  templateUrl: './upload-target.html',
  styleUrl: './upload-target.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class UploadTarget {
  readonly pending = input(0);
  readonly chosen = output<File[]>();

  private readonly input = viewChild.required<ElementRef<HTMLInputElement>>('picker');

  protected open(): void {
    this.input().nativeElement.click();
  }

  protected pick(event: Event): void {
    const picker = event.target as HTMLInputElement;
    const files = Array.from(picker.files ?? []);
    picker.value = '';
    if (files.length > 0) {
      this.chosen.emit(files);
    }
  }
}
