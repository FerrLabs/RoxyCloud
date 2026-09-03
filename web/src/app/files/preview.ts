import {
  ChangeDetectionStrategy,
  Component,
  DestroyRef,
  ElementRef,
  afterNextRender,
  computed,
  inject,
  input,
  output,
  signal,
  viewChild,
} from '@angular/core';
import { DomSanitizer, type SafeResourceUrl } from '@angular/platform-browser';
import { formatSize, type Node } from '../node';
import { PLATFORM } from '../platform';
import { PREVIEW_LIMIT, mimeOf, previewKind } from './preview-kind';

type Shown =
  | { kind: 'loading' }
  | { kind: 'image'; url: SafeResourceUrl }
  | { kind: 'text'; body: string }
  | { kind: 'pdf'; url: SafeResourceUrl }
  | { kind: 'none'; reason: string }
  | { kind: 'failed'; reason: string };

@Component({
  selector: 'rx-preview',
  templateUrl: './preview.html',
  styleUrl: './preview.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Preview {
  readonly node = input.required<Node>();
  readonly path = input.required<string>();
  readonly closed = output<void>();
  readonly downloaded = output<void>();

  private readonly platform = inject(PLATFORM);
  private readonly sanitizer = inject(DomSanitizer);
  private readonly dialog = viewChild.required<ElementRef<HTMLDialogElement>>('dialog');
  private objectUrl: string | null = null;

  protected readonly shown = signal<Shown>({ kind: 'loading' });
  protected readonly size = formatSize;

  protected readonly loading = computed(() => this.shown().kind === 'loading');
  protected readonly image = computed(() => {
    const shown = this.shown();
    return shown.kind === 'image' ? shown.url : null;
  });
  protected readonly pdf = computed(() => {
    const shown = this.shown();
    return shown.kind === 'pdf' ? shown.url : null;
  });
  protected readonly text = computed(() => {
    const shown = this.shown();
    return shown.kind === 'text' ? shown.body : null;
  });
  protected readonly message = computed(() => {
    const shown = this.shown();
    return shown.kind === 'none' || shown.kind === 'failed' ? shown.reason : null;
  });
  protected readonly failed = computed(() => this.shown().kind === 'failed');

  constructor() {
    afterNextRender(() => {
      this.dialog().nativeElement.showModal();
      void this.load();
    });
    inject(DestroyRef).onDestroy(() => this.revoke());
  }

  protected close(): void {
    this.dialog().nativeElement.close();
    this.closed.emit();
  }

  private async load(): Promise<void> {
    const node = this.node();
    const kind = previewKind(node.name, node.size);
    if (kind === 'none') {
      this.shown.set({
        kind: 'none',
        reason:
          node.size > PREVIEW_LIMIT
            ? `This file is ${formatSize(node.size)}, too large to open here.`
            : 'There is no preview for this kind of file.',
      });
      return;
    }

    try {
      const blob = await this.platform.read(this.path());
      if (kind === 'text') {
        this.shown.set({ kind: 'text', body: await blob.text() });
        return;
      }
      this.objectUrl = URL.createObjectURL(blob.slice(0, blob.size, mimeOf(node.name)));
      this.shown.set({
        kind,
        url: this.sanitizer.bypassSecurityTrustResourceUrl(this.objectUrl),
      });
    } catch (cause: unknown) {
      this.shown.set({ kind: 'failed', reason: String(cause) });
    }
  }

  private revoke(): void {
    if (this.objectUrl !== null) {
      URL.revokeObjectURL(this.objectUrl);
      this.objectUrl = null;
    }
  }
}
