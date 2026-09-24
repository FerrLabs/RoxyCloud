import {
  ChangeDetectionStrategy,
  Component,
  inject,
  input,
  output,
  resource,
  signal,
} from '@angular/core';
import { formatSize, type Node, type Trashed } from '../node';
import { PLATFORM, RequestFailed } from '../platform';
import { Confirm } from '../shared/confirm';
import { formatMoment } from '../version';

@Component({
  selector: 'rx-trash-panel',
  imports: [Confirm],
  templateUrl: './trash-panel.html',
  styleUrl: './trash-panel.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class TrashPanel {
  private readonly platform = inject(PLATFORM);

  readonly version = input(0);
  readonly canChange = input.required<boolean>();
  readonly closed = output<void>();
  readonly restored = output<void>();

  protected readonly trashed = resource({
    params: () => ({ version: this.version() }),
    loader: () => this.platform.listTrash?.() ?? Promise.resolve([]),
  });

  protected readonly busy = signal(false);
  protected readonly failure = signal<string | null>(null);
  protected readonly announcement = signal<string | null>(null);
  protected readonly doomed = signal<Node | null>(null);
  protected readonly emptying = signal(false);

  protected describe(node: Trashed): string {
    const parts = [node.kind === 'directory' ? 'Folder' : formatSize(node.size)];
    if (node.deleted_at !== undefined) {
      parts.push(`deleted ${formatMoment(node.deleted_at)}`);
    }
    if (node.expires_at !== undefined) {
      parts.push(`goes for good ${formatMoment(node.expires_at)}`);
    }
    return parts.join(', ');
  }

  protected async empty(): Promise<void> {
    this.emptying.set(false);
    const clear = this.platform.emptyTrash;
    if (clear === undefined) {
      return;
    }
    await this.attempt(async () => {
      await clear();
      this.announcement.set('Emptied the trash');
    });
  }

  protected async restore(node: Node): Promise<void> {
    const bring = this.platform.restoreFromTrash;
    if (bring === undefined) {
      return;
    }
    await this.attempt(async () => {
      try {
        await bring(node.id);
      } catch (cause: unknown) {
        if (cause instanceof RequestFailed && cause.status === 409) {
          throw new Error(`${cause.message}. Rename or move that one, then restore again.`);
        }
        throw cause;
      }
      this.announcement.set(`Restored ${node.name} to where it was`);
      this.restored.emit();
    });
  }

  protected async purge(node: Node): Promise<void> {
    this.doomed.set(null);
    const drop = this.platform.purgeFromTrash;
    if (drop === undefined) {
      return;
    }
    await this.attempt(async () => {
      await drop(node.id);
      this.announcement.set(`Deleted ${node.name} for good`);
    });
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
      this.trashed.reload();
    } catch (cause: unknown) {
      this.failure.set(cause instanceof Error ? cause.message : String(cause));
    } finally {
      this.busy.set(false);
    }
  }
}
