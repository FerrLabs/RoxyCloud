import { ChangeDetectionStrategy, Component, inject, input, output, resource, signal } from '@angular/core';
import { PLATFORM } from '../platform';
import { describeLink, type Share } from '../share';
import { Confirm } from '../shared/confirm';

@Component({
  selector: 'rx-shares-panel',
  imports: [Confirm],
  templateUrl: './shares-panel.html',
  styleUrl: './shares-panel.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class SharesPanel {
  private readonly platform = inject(PLATFORM);

  readonly version = input(0);
  readonly closed = output<void>();

  protected readonly links = resource({
    params: () => ({ version: this.version() }),
    loader: () => this.platform.listShares?.() ?? Promise.resolve([]),
  });

  protected readonly doomed = signal<Share | null>(null);
  protected readonly failure = signal<string | null>(null);

  protected readonly describe = describeLink;

  protected async revoke(share: Share): Promise<void> {
    this.doomed.set(null);
    const drop = this.platform.revokeShare;
    if (drop === undefined) {
      return;
    }

    this.failure.set(null);
    try {
      await drop(share.id);
      this.links.reload();
    } catch (cause: unknown) {
      this.failure.set(cause instanceof Error ? cause.message : String(cause));
    }
  }
}
