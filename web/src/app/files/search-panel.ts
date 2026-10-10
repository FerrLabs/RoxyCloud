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
import { linkTo, parentOf } from '../folder';
import { formatSize, type Hit } from '../node';
import { PLATFORM } from '../platform';

@Component({
  selector: 'rx-search-panel',
  templateUrl: './search-panel.html',
  styleUrl: './search-panel.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class SearchPanel {
  private readonly platform = inject(PLATFORM);
  private readonly field = viewChild.required<ElementRef<HTMLInputElement>>('field');

  readonly closed = output<void>();
  readonly chosen = output<string[]>();

  protected readonly term = signal('');
  protected readonly searched = signal<string | null>(null);
  protected readonly hits = signal<Hit[]>([]);
  protected readonly busy = signal(false);
  protected readonly failure = signal<string | null>(null);

  constructor() {
    afterNextRender(() => this.field().nativeElement.focus());
  }

  protected async search(): Promise<void> {
    const term = this.term().trim();
    const search = this.platform.search;
    if (term.length === 0 || search === undefined) {
      return;
    }
    this.busy.set(true);
    this.failure.set(null);
    try {
      this.hits.set(await search(term));
      this.searched.set(term);
    } catch (cause: unknown) {
      this.failure.set(`searching failed: ${cause instanceof Error ? cause.message : cause}`);
    } finally {
      this.busy.set(false);
    }
  }

  protected where(hit: Hit): string {
    return parentOf(hit.path) || 'your files';
  }

  protected describe(hit: Hit): string {
    return hit.kind === 'directory' ? 'Folder' : formatSize(hit.size);
  }

  protected choose(hit: Hit): void {
    const folder = hit.kind === 'directory' ? hit.path : (parentOf(hit.path) ?? '');
    this.chosen.emit(linkTo(folder));
  }

  protected onKeydown(event: KeyboardEvent): void {
    if (event.key === 'Escape') {
      event.preventDefault();
      this.closed.emit();
    }
  }
}
