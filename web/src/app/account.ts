import { Injectable, computed, inject, signal } from '@angular/core';
import { PLATFORM } from './platform';

export type Role = 'admin' | 'member' | 'reader';

export type Account = {
  id: string;
  email: string;
  display_name: string;
  role: Role;
};

@Injectable({ providedIn: 'root' })
export class Session {
  private readonly platform = inject(PLATFORM);

  readonly account = signal<Account | null>(null);

  readonly canWrite = computed(() => {
    const role = this.account()?.role;
    return role === 'admin' || role === 'member';
  });

  readonly isReader = computed(() => this.account()?.role === 'reader');

  async load(): Promise<void> {
    this.account.set(await this.platform.account());
  }

  forget(): void {
    this.account.set(null);
  }
}
