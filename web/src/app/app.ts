import { ChangeDetectionStrategy, Component, inject, signal } from '@angular/core';
import { toSignal } from '@angular/core/rxjs-interop';
import { NavigationEnd, Router, RouterOutlet } from '@angular/router';
import { filter, map, startWith } from 'rxjs';
import { AccountMenu } from './account/account-menu';
import { AppPasswords } from './account/app-passwords';
import { ChangePassword } from './account/change-password';
import { Session } from './account';
import type { Credentials } from './login-form/credentials';
import { LoginForm } from './login-form/login-form';
import { PLATFORM, RequestFailed } from './platform';
import { SHARE_PREFIX } from './share';

@Component({
  selector: 'rx-root',
  imports: [AccountMenu, AppPasswords, ChangePassword, LoginForm, RouterOutlet],
  templateUrl: './app.html',
  styleUrl: './app.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class App {
  private readonly platform = inject(PLATFORM);
  private readonly router = inject(Router);
  protected readonly session = inject(Session);

  protected readonly recipient = toSignal(
    this.router.events.pipe(
      filter((event) => event instanceof NavigationEnd),
      map(() => this.router.url.startsWith(`${SHARE_PREFIX}/`)),
      startWith(this.router.url.startsWith(`${SHARE_PREFIX}/`)),
    ),
    { initialValue: false },
  );

  protected readonly sourceUrl = ROXYCLOUD_SOURCE_URL;
  protected readonly connected = signal(this.platform.authenticated());
  protected readonly error = signal<string | null>(null);
  protected readonly busy = signal(false);
  protected readonly changing = signal(false);
  protected readonly notice = signal<string | null>(null);
  protected readonly listingAppPasswords = signal(false);

  constructor() {
    if (this.connected()) {
      void this.start();
    }
  }

  private async start(): Promise<void> {
    try {
      await this.session.load();
    } catch (cause: unknown) {
      if (cause instanceof RequestFailed && (cause.status === 401 || cause.status === 403)) {
        void this.signOut();
        return;
      }
      const message = cause instanceof Error ? cause.message : String(cause);
      this.error.set(`Your account did not load: ${message}`);
    }
  }

  protected async signOut(): Promise<void> {
    try {
      await this.platform.signOut();
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      this.error.set(`Signed out here, but the app kept its session: ${message}`);
    }
    this.session.forget();
    this.connected.set(false);
    this.notice.set(null);
    void this.router.navigate(['/']);
  }

  protected changePassword(): void {
    this.notice.set(null);
    this.changing.set(true);
  }

  protected noteChanged(): void {
    this.changing.set(false);
    this.notice.set('Your password has been changed.');
  }

  protected async signIn(credentials: Credentials): Promise<void> {
    this.error.set(null);
    this.busy.set(true);
    try {
      await this.platform.login(credentials.email, credentials.password);
      this.connected.set(true);
      await this.session.load();
    } catch (cause: unknown) {
      this.error.set(String(cause));
    } finally {
      this.busy.set(false);
    }
  }
}
