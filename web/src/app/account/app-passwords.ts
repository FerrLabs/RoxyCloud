import {
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  afterNextRender,
  inject,
  output,
  resource,
  signal,
  viewChild,
} from '@angular/core';
import { Session } from '../account';
import { PLATFORM } from '../platform';
import { Confirm } from '../shared/confirm';
import { davUrl, describeUse, type AppPassword, type MintedPassword } from './app-password';

@Component({
  selector: 'rx-app-passwords',
  imports: [Confirm],
  templateUrl: './app-passwords.html',
  styleUrl: './app-passwords.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class AppPasswords {
  private readonly platform = inject(PLATFORM);
  private readonly session = inject(Session);

  readonly dismissed = output<void>();

  private readonly dialog = viewChild.required<ElementRef<HTMLDialogElement>>('dialog');

  protected readonly account = this.session.account;
  protected readonly davUrl = davUrl();
  protected readonly describe = describeUse;

  protected readonly name = signal('');
  protected readonly busy = signal(false);
  protected readonly failure = signal<string | null>(null);
  protected readonly minted = signal<MintedPassword | null>(null);
  protected readonly copied = signal(false);
  protected readonly doomed = signal<AppPassword | null>(null);
  private readonly version = signal(0);

  protected readonly passwords = resource({
    params: () => ({ version: this.version() }),
    loader: () => this.platform.listAppPasswords?.() ?? Promise.resolve([]),
  });

  constructor() {
    afterNextRender(() => this.dialog().nativeElement.showModal());
  }

  protected async mint(): Promise<void> {
    const mint = this.platform.mintAppPassword;
    const name = this.name().trim();
    if (mint === undefined || name.length === 0 || this.busy()) {
      return;
    }

    this.failure.set(null);
    this.busy.set(true);
    try {
      this.minted.set(await mint(name));
      this.copied.set(false);
      this.name.set('');
      this.version.update((count) => count + 1);
    } catch (cause: unknown) {
      this.failure.set(reasonFor(cause));
    } finally {
      this.busy.set(false);
    }
  }

  protected async revoke(password: AppPassword): Promise<void> {
    this.doomed.set(null);
    const revoke = this.platform.revokeAppPassword;
    if (revoke === undefined) {
      return;
    }

    this.failure.set(null);
    try {
      await revoke(password.id);
      this.version.update((count) => count + 1);
    } catch (cause: unknown) {
      this.failure.set(reasonFor(cause));
    }
  }

  protected async copy(): Promise<void> {
    const secret = this.minted()?.secret;
    if (secret === undefined) {
      return;
    }
    try {
      await navigator.clipboard.writeText(secret);
      this.copied.set(true);
    } catch {
      this.copied.set(false);
      this.failure.set('Copying failed. Select the password and copy it by hand.');
    }
  }

  protected done(): void {
    this.failure.set(null);
    this.minted.set(null);
  }

  protected dismiss(): void {
    this.dialog().nativeElement.close();
    this.dismissed.emit();
  }
}

function reasonFor(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
