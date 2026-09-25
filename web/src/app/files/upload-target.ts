import {
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  inject,
  input,
  output,
  viewChild,
} from '@angular/core';
import { fromFiles, type Outgoing } from '../outgoing';
import { PLATFORM } from '../platform';

@Component({
  selector: 'rx-upload-target',
  templateUrl: './upload-target.html',
  styleUrl: './upload-target.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class UploadTarget {
  private readonly platform = inject(PLATFORM);

  readonly pending = input(0);
  readonly chosen = output<Outgoing[]>();
  readonly failed = output<string>();

  private readonly input = viewChild.required<ElementRef<HTMLInputElement>>('picker');

  protected async open(): Promise<void> {
    const pickNatively = this.platform.pickUploads;
    if (pickNatively === undefined) {
      this.input().nativeElement.click();
      return;
    }
    try {
      const items = await pickNatively();
      if (items.length > 0) {
        this.chosen.emit(items);
      }
    } catch (cause: unknown) {
      this.failed.emit(cause instanceof Error ? cause.message : String(cause));
    }
  }

  protected pick(event: Event): void {
    const picker = event.target as HTMLInputElement;
    const files = Array.from(picker.files ?? []);
    picker.value = '';
    const upload = this.platform.upload;
    if (upload !== undefined && files.length > 0) {
      this.chosen.emit(fromFiles(upload, files));
    }
  }
}
