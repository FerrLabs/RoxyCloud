import {
  ChangeDetectionStrategy,
  Component,
  computed,
  effect,
  inject,
  resource,
  signal,
} from '@angular/core';
import { Router } from '@angular/router';
import { Session, describeRole, type Role } from '../account';
import { PLATFORM } from '../platform';
import { Confirm } from '../shared/confirm';
import { AccountPassword } from './account-password';
import { NewAccountDialog } from './new-account';
import { QuotaDialog } from './quota';
import { ROLES, describeUsage, type ManagedAccount } from './managed';

@Component({
  selector: 'rx-accounts',
  imports: [AccountPassword, Confirm, NewAccountDialog, QuotaDialog],
  templateUrl: './accounts.html',
  styleUrl: './accounts.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Accounts {
  private readonly platform = inject(PLATFORM);
  private readonly router = inject(Router);
  private readonly session = inject(Session);

  protected readonly roles = ROLES;
  protected readonly describe = describeRole;
  protected readonly usage = describeUsage;
  protected readonly me = computed(() => this.session.account()?.id ?? null);

  private readonly version = signal(0);
  protected readonly accounts = resource({
    params: () => (this.session.isAdmin() ? { version: this.version() } : undefined),
    loader: () => this.platform.listAccounts?.() ?? Promise.resolve([]),
  });

  protected readonly failure = signal<string | null>(null);
  protected readonly announcement = signal<string | null>(null);
  protected readonly adding = signal(false);
  protected readonly disabling = signal<ManagedAccount | null>(null);
  protected readonly quota = signal<ManagedAccount | null>(null);
  protected readonly resetting = signal<ManagedAccount | null>(null);
  protected readonly password = signal<{ heading: string; secret: string } | null>(null);

  constructor() {
    effect(() => {
      const account = this.session.account();
      if (account !== null && account.role !== 'admin') {
        void this.router.navigate(['/']);
      }
    });
  }

  protected async setRole(account: ManagedAccount, event: Event): Promise<void> {
    const select = event.target as HTMLSelectElement;
    const role = select.value as Role;
    const changed = await this.attempt(`changing ${account.email}`, async () => {
      await this.platform.setRole?.(account.id, role);
      this.announcement.set(`${account.email} is now ${describeRole(role).toLowerCase()}`);
    });
    if (!changed) {
      select.value = account.role;
    }
  }

  protected async setDisabled(account: ManagedAccount, disabled: boolean): Promise<void> {
    this.disabling.set(null);
    await this.attempt(`changing ${account.email}`, async () => {
      await this.platform.setDisabled?.(account.id, disabled);
      this.announcement.set(
        disabled ? `${account.email} is disabled` : `${account.email} can sign in again`,
      );
    });
  }

  protected async unlock(account: ManagedAccount): Promise<void> {
    await this.attempt(`unlocking ${account.email}`, async () => {
      await this.platform.unlockAccount?.(account.id);
      this.announcement.set(`${account.email} may try again`);
    });
  }

  protected async setQuota(account: ManagedAccount, bytes: number): Promise<void> {
    this.quota.set(null);
    await this.attempt(`changing the quota of ${account.email}`, async () => {
      await this.platform.setQuota?.(account.id, bytes);
      this.announcement.set(`The quota of ${account.email} is set`);
    });
  }

  protected async resetPassword(account: ManagedAccount, secret: string): Promise<void> {
    this.resetting.set(null);
    await this.attempt(`resetting the password of ${account.email}`, async () => {
      await this.platform.resetPassword?.(account.id, secret);
      this.password.set({ heading: `New password for ${account.email}`, secret });
    });
  }

  protected async create(account: {
    email: string;
    display_name: string;
    password: string;
    role: Role;
  }): Promise<void> {
    this.adding.set(false);
    await this.attempt(`creating ${account.email}`, async () => {
      await this.platform.createAccount?.(account);
      this.password.set({
        heading: `Password for ${account.email}`,
        secret: account.password,
      });
    });
  }

  private async attempt(what: string, run: () => Promise<void>): Promise<boolean> {
    this.failure.set(null);
    this.announcement.set(null);
    try {
      await run();
      this.version.update((count) => count + 1);
      return true;
    } catch (cause: unknown) {
      this.failure.set(`${what} failed: ${cause instanceof Error ? cause.message : String(cause)}`);
      return false;
    }
  }
}
