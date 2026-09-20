import { ChangeDetectionStrategy, Component, inject, input, output } from '@angular/core';
import { FormBuilder, ReactiveFormsModule, Validators } from '@angular/forms';
import { PLATFORM } from '../platform';
import type { Credentials } from './credentials';

@Component({
  selector: 'rx-login-form',
  imports: [ReactiveFormsModule],
  templateUrl: './login-form.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class LoginForm {
  private readonly platform = inject(PLATFORM);

  readonly busy = input(false);
  readonly submitted = output<Credentials>();

  protected readonly asks = this.platform.server !== undefined;

  protected readonly form = inject(FormBuilder).nonNullable.group({
    server: [this.platform.server?.() ?? '', this.asks ? [Validators.required] : []],
    email: ['', [Validators.required, Validators.email]],
    password: ['', Validators.required],
  });

  protected submit(): void {
    if (this.form.invalid) {
      return;
    }
    const { server, ...credentials } = this.form.getRawValue();
    this.submitted.emit(this.asks ? { ...credentials, server } : credentials);
  }
}
