import {
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  afterNextRender,
  output,
  signal,
  viewChild,
} from '@angular/core';
import { describeRole, type Role } from '../account';
import { ROLES, generatePassword, type NewAccount } from './managed';

@Component({
  selector: 'rx-new-account',
  templateUrl: './new-account.html',
  styleUrl: './account-password.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class NewAccountDialog {
  readonly submitted = output<NewAccount>();
  readonly dismissed = output<void>();

  private readonly dialog = viewChild.required<ElementRef<HTMLDialogElement>>('dialog');

  protected readonly roles = ROLES;
  protected readonly describe = describeRole;

  protected readonly email = signal('');
  protected readonly displayName = signal('');
  protected readonly role = signal<Role>('member');
  protected readonly password = signal(generatePassword());

  constructor() {
    afterNextRender(() => this.dialog().nativeElement.showModal());
  }

  protected submit(): void {
    const email = this.email().trim();
    const displayName = this.displayName().trim();
    if (email.length === 0 || displayName.length === 0 || this.password().length === 0) {
      return;
    }
    this.dialog().nativeElement.close();
    this.submitted.emit({
      email,
      display_name: displayName,
      password: this.password(),
      role: this.role(),
    });
  }

  protected dismiss(): void {
    this.dialog().nativeElement.close();
    this.dismissed.emit();
  }
}
