import {
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  computed,
  effect,
  inject,
  resource,
  signal,
  viewChildren,
} from '@angular/core';
import { toSignal } from '@angular/core/rxjs-interop';
import { ActivatedRoute, Router, type UrlSegment } from '@angular/router';
import { Session } from '../account';
import { childOf } from '../folder';
import { byKindThenName, formatDate, formatSize, type Node } from '../node';
import { PLATFORM } from '../platform';
import { Confirm } from '../shared/confirm';
import { Prompt } from '../shared/prompt';
import { Breadcrumb } from './breadcrumb';
import { Preview } from './preview';
import { UploadTarget } from './upload-target';

@Component({
  selector: 'rx-file-browser',
  imports: [Breadcrumb, Confirm, Preview, Prompt, UploadTarget],
  templateUrl: './file-browser.html',
  styleUrl: './file-browser.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class FileBrowser {
  private readonly platform = inject(PLATFORM);
  private readonly router = inject(Router);

  private readonly segments = toSignal(inject(ActivatedRoute).url, {
    initialValue: [] as UrlSegment[],
  });

  private readonly entries = viewChildren<ElementRef<HTMLElement>>('entry');

  protected readonly path = computed(() =>
    this.segments()
      .map((segment) => segment.path)
      .join('/'),
  );

  protected readonly listing = resource({
    params: () => ({ path: this.path() }),
    loader: ({ params }) => this.platform.listFolder(`/${params.path}`),
  });

  protected readonly nodes = computed(() =>
    [...(this.listing.value() ?? [])].sort(byKindThenName),
  );

  private readonly session = inject(Session);

  protected readonly canWrite = this.session.canWrite;
  protected readonly canUpload = computed(
    () => this.platform.upload !== undefined && this.canWrite(),
  );
  protected readonly dragging = signal(false);
  protected readonly pending = signal(0);
  protected readonly announcement = signal<string | null>(null);
  protected readonly failure = signal<string | null>(null);
  protected readonly doomed = signal<Node | null>(null);
  protected readonly renaming = signal<Node | null>(null);
  protected readonly opened = signal<Node | null>(null);

  protected readonly size = formatSize;
  protected readonly date = formatDate;

  constructor() {
    effect(() => {
      this.path();
      this.announcement.set(null);
      this.failure.set(null);
      this.opened.set(null);
      this.renaming.set(null);
    });
  }

  protected open(node: Node): void {
    if (node.kind === 'directory') {
      void this.router.navigate(['/', ...this.path().split('/').filter(Boolean), node.name]);
      return;
    }
    this.opened.set(node);
  }

  protected pathOf(node: Node): string {
    return childOf(this.path(), node.name);
  }

  protected async download(node: Node): Promise<void> {
    await this.attempt(`downloading ${node.name}`, async () => {
      const saved = await this.platform.download(childOf(this.path(), node.name), node.name);
      this.announcement.set(saved ? `Saved ${node.name} to ${saved}` : `Downloaded ${node.name}`);
    });
  }

  protected async rename(node: Node, destination: string): Promise<void> {
    this.renaming.set(null);
    const to = childOf(this.path(), destination);
    await this.attempt(`renaming ${node.name}`, async () => {
      await this.platform.rename(this.pathOf(node), to);
      this.announcement.set(
        destination.includes('/') ? `Moved ${node.name} to ${to}` : `Renamed ${node.name}`,
      );
      this.listing.reload();
    });
  }

  protected async remove(node: Node): Promise<void> {
    this.doomed.set(null);
    await this.attempt(`deleting ${node.name}`, async () => {
      await this.platform.remove(childOf(this.path(), node.name));
      this.announcement.set(`Moved ${node.name} to the trash`);
      this.listing.reload();
    });
  }

  protected async upload(files: File[]): Promise<void> {
    const send = this.platform.upload;
    if (send === undefined) {
      return;
    }

    this.pending.set(files.length);
    let sent = 0;
    for (const file of files) {
      const done = await this.attempt(`uploading ${file.name}`, async () => {
        await send(childOf(this.path(), file.name), file);
      });
      if (done) {
        sent += 1;
      }
      this.pending.update((left) => left - 1);
    }

    if (sent > 0) {
      this.announcement.set(sent === 1 ? 'Uploaded 1 file' : `Uploaded ${sent} files`);
      this.listing.reload();
    }
  }

  protected onDragOver(event: DragEvent): void {
    if (!this.canUpload()) {
      return;
    }
    event.preventDefault();
    this.dragging.set(true);
  }

  protected onDragLeave(): void {
    this.dragging.set(false);
  }

  protected onDrop(event: DragEvent): void {
    if (!this.canUpload()) {
      return;
    }
    event.preventDefault();
    this.dragging.set(false);
    const files = Array.from(event.dataTransfer?.files ?? []);
    if (files.length > 0) {
      void this.upload(files);
    }
  }

  protected onKeydown(event: KeyboardEvent, index: number, node: Node): void {
    const last = this.nodes().length - 1;
    switch (event.key) {
      case 'ArrowDown':
        this.focus(Math.min(index + 1, last));
        break;
      case 'ArrowUp':
        this.focus(Math.max(index - 1, 0));
        break;
      case 'Home':
        this.focus(0);
        break;
      case 'End':
        this.focus(last);
        break;
      case 'Delete':
        if (this.canWrite()) {
          this.doomed.set(node);
        }
        break;
      case 'F2':
        if (this.canWrite()) {
          this.renaming.set(node);
        }
        break;
      default:
        return;
    }
    event.preventDefault();
  }

  private focus(index: number): void {
    this.entries().at(index)?.nativeElement.focus();
  }

  private async attempt(what: string, run: () => Promise<void>): Promise<boolean> {
    try {
      await run();
      return true;
    } catch (cause: unknown) {
      this.announcement.set(null);
      this.failure.set(`${what} failed: ${reasonFor(cause)}`);
      return false;
    }
  }
}

function reasonFor(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
