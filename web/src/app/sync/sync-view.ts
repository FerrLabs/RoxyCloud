import {
  ChangeDetectionStrategy,
  Component,
  DestroyRef,
  computed,
  inject,
  signal,
} from '@angular/core';
import { Router } from '@angular/router';
import { Session } from '../account';
import { linkTo } from '../folder';
import { PLATFORM } from '../platform';
import {
  needsAttention,
  summarise,
  type SyncCommand,
  type SyncReport,
  type Syncing,
} from './syncing';

const FOLDER_KEY = 'roxycloud.sync-folder';

@Component({
  selector: 'rx-sync-view',
  templateUrl: './sync-view.html',
  styleUrl: './sync-view.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class SyncView {
  private readonly platform = inject(PLATFORM);
  private readonly session = inject(Session);

  protected readonly syncing = signal<Syncing | null>(null);
  protected readonly failure = signal<string | null>(null);
  protected readonly busy = signal(false);
  protected readonly remembered = signal<string | null>(null);

  protected readonly running = computed(() => {
    const state = this.syncing()?.status.state;
    return state !== undefined && state !== 'stopped';
  });
  protected readonly paused = computed(() => this.syncing()?.status.state === 'paused');
  protected readonly report = computed<SyncReport | null>(() => this.syncing()?.last ?? null);
  protected readonly summary = computed(() => {
    const report = this.report();
    return report === null ? null : summarise(report);
  });
  protected readonly attention = computed(() => {
    const report = this.report();
    return report !== null && needsAttention(report);
  });

  constructor() {
    if (this.platform.startSync === undefined) {
      void inject(Router).navigate(linkTo(''));
      return;
    }

    const destroyRef = inject(DestroyRef);
    void this.follow(destroyRef);
  }

  private async follow(destroyRef: DestroyRef): Promise<void> {
    const unlisten = await this.platform.watchSync?.((syncing) => this.syncing.set(syncing));
    if (unlisten !== undefined) {
      if (destroyRef.destroyed) {
        unlisten();
      } else {
        destroyRef.onDestroy(unlisten);
      }
    }

    await this.refresh();
    this.remembered.set(this.readRemembered());
  }

  protected describe(): string {
    const status = this.syncing()?.status;
    switch (status?.state) {
      case 'idle':
        return 'Watching for changes';
      case 'syncing':
        return 'Syncing';
      case 'synced':
        return 'In step with the server';
      case 'failed':
        return `The last pass failed: ${status.reason}`;
      case 'paused':
        return 'Paused, changes wait until you resume';
      case 'stopped':
        return 'Stopped';
      default:
        return '';
    }
  }

  protected async choose(): Promise<void> {
    await this.run(async () => {
      const folder = await this.platform.pickFolder?.();
      if (folder !== null && folder !== undefined) {
        await this.begin(folder);
      }
    });
  }

  protected async start(folder: string): Promise<void> {
    await this.run(() => this.begin(folder));
  }

  private async begin(folder: string): Promise<void> {
    await this.platform.startSync?.(folder);
    this.remember(folder);
    await this.refresh();
  }

  private async refresh(): Promise<void> {
    const current = await this.platform.syncStatus?.();
    if (current !== null && current !== undefined) {
      this.syncing.set(current);
    }
  }

  protected async control(command: SyncCommand): Promise<void> {
    await this.run(() => this.platform.controlSync?.(command) ?? Promise.resolve());
  }

  private async run(action: () => Promise<void>): Promise<void> {
    this.failure.set(null);
    this.busy.set(true);
    try {
      await action();
    } catch (cause: unknown) {
      this.failure.set(cause instanceof Error ? cause.message : String(cause));
    } finally {
      this.busy.set(false);
    }
  }

  private storageKey(): string | null {
    const email = this.session.account()?.email;
    return email === undefined ? null : `${FOLDER_KEY}:${email}`;
  }

  private readRemembered(): string | null {
    const key = this.storageKey();
    if (key === null) {
      return null;
    }
    try {
      return localStorage.getItem(key);
    } catch {
      return null;
    }
  }

  private remember(folder: string): void {
    const key = this.storageKey();
    this.remembered.set(folder);
    if (key === null) {
      return;
    }
    try {
      localStorage.setItem(key, folder);
    } catch {
      return;
    }
  }
}
