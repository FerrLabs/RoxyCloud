import {
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  afterNextRender,
  inject,
  input,
  output,
  resource,
  signal,
  viewChild,
} from '@angular/core';
import { formatSize, type Node } from '../node';
import { PLATFORM } from '../platform';
import { Confirm } from '../shared/confirm';
import { formatMoment, type Version } from '../version';

@Component({
  selector: 'rx-versions-dialog',
  imports: [Confirm],
  templateUrl: './versions-dialog.html',
  styleUrl: './versions-dialog.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class VersionsDialog {
  private readonly platform = inject(PLATFORM);

  readonly node = input.required<Node>();
  readonly path = input.required<string>();
  readonly canRestore = input.required<boolean>();
  readonly dismissed = output<void>();
  readonly restored = output<void>();

  private readonly dialog = viewChild.required<ElementRef<HTMLDialogElement>>('dialog');

  protected readonly busy = signal(false);
  protected readonly failure = signal<string | null>(null);
  protected readonly announcement = signal<string | null>(null);
  protected readonly restoring = signal<Version | null>(null);

  protected readonly versions = resource({
    params: () => ({ path: this.path() }),
    loader: ({ params }) => this.platform.listVersions?.(params.path) ?? Promise.resolve([]),
  });

  protected readonly size = formatSize;
  protected readonly moment = formatMoment;

  constructor() {
    afterNextRender(() => this.dialog().nativeElement.showModal());
  }

  protected async download(version: Version): Promise<void> {
    const get = this.platform.downloadVersion;
    if (get === undefined) {
      return;
    }
    await this.attempt(async () => {
      await get(this.path(), version.id, this.node().name);
      this.announcement.set(`Downloaded the version from ${formatMoment(version.created_at)}.`);
    });
  }

  protected async restore(version: Version): Promise<void> {
    this.restoring.set(null);
    const put = this.platform.restoreVersion;
    if (put === undefined) {
      return;
    }
    await this.attempt(async () => {
      await put(this.path(), version.id);
      this.announcement.set(
        `Restored the version from ${formatMoment(version.created_at)}. What it replaced is now a version too.`,
      );
      this.versions.reload();
      this.restored.emit();
    });
  }

  protected dismiss(): void {
    this.dialog().nativeElement.close();
    this.dismissed.emit();
  }

  private async attempt(run: () => Promise<void>): Promise<void> {
    if (this.busy()) {
      return;
    }
    this.failure.set(null);
    this.announcement.set(null);
    this.busy.set(true);
    try {
      await run();
    } catch (cause: unknown) {
      this.failure.set(cause instanceof Error ? cause.message : String(cause));
    } finally {
      this.busy.set(false);
    }
  }
}
