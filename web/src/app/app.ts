import { ChangeDetectionStrategy, Component, inject, signal } from '@angular/core';
import { LoginForm } from './login-form/login-form';
import type { Credentials } from './login-form/credentials';
import { describeNode, type Node } from './node';
import { PLATFORM } from './platform';

@Component({
  selector: 'rx-root',
  imports: [LoginForm],
  templateUrl: './app.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class App {
  private readonly platform = inject(PLATFORM);

  protected readonly sourceUrl = ROXYCLOUD_SOURCE_URL;
  protected readonly describe = describeNode;
  protected readonly nodes = signal<Node[] | null>(null);
  protected readonly error = signal<string | null>(null);
  protected readonly busy = signal(false);

  constructor() {
    if (this.platform.authenticated()) {
      void this.browse();
    }
  }

  protected async signIn(credentials: Credentials): Promise<void> {
    this.error.set(null);
    this.busy.set(true);
    try {
      await this.platform.login(credentials.email, credentials.password);
      await this.browse();
    } catch (cause: unknown) {
      this.error.set(String(cause));
    } finally {
      this.busy.set(false);
    }
  }

  private async browse(): Promise<void> {
    try {
      this.nodes.set(await this.platform.listFolder('/'));
    } catch (cause: unknown) {
      this.error.set(String(cause));
    }
  }
}
