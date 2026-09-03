import { ChangeDetectionStrategy, Component, inject, signal } from '@angular/core';
import { RouterOutlet } from '@angular/router';
import type { Credentials } from './login-form/credentials';
import { LoginForm } from './login-form/login-form';
import { PLATFORM } from './platform';

@Component({
  selector: 'rx-root',
  imports: [LoginForm, RouterOutlet],
  templateUrl: './app.html',
  styleUrl: './app.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class App {
  private readonly platform = inject(PLATFORM);

  protected readonly sourceUrl = ROXYCLOUD_SOURCE_URL;
  protected readonly connected = signal(this.platform.authenticated());
  protected readonly error = signal<string | null>(null);
  protected readonly busy = signal(false);

  protected async signIn(credentials: Credentials): Promise<void> {
    this.error.set(null);
    this.busy.set(true);
    try {
      await this.platform.login(credentials.email, credentials.password);
      this.connected.set(true);
    } catch (cause: unknown) {
      this.error.set(String(cause));
    } finally {
      this.busy.set(false);
    }
  }
}
