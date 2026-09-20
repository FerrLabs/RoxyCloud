import {
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  HostListener,
  computed,
  inject,
  output,
  signal,
} from '@angular/core';
import { Router } from '@angular/router';
import { Session, describeRole } from '../account';
import { PLATFORM } from '../platform';

@Component({
  selector: 'rx-account-menu',
  templateUrl: './account-menu.html',
  styleUrl: './account-menu.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class AccountMenu {
  private readonly host = inject<ElementRef<HTMLElement>>(ElementRef);
  private readonly platform = inject(PLATFORM);
  private readonly router = inject(Router);
  private readonly session = inject(Session);

  readonly changingPassword = output<void>();
  readonly checkingForUpdates = output<void>();
  readonly openingAppPasswords = output<void>();
  readonly signedOut = output<void>();

  protected readonly canChangePassword = this.platform.changePassword !== undefined;
  protected readonly canUpdate = this.platform.checkUpdate !== undefined;
  protected readonly canMintAppPasswords = this.platform.mintAppPassword !== undefined;
  protected readonly canAdminister = computed(
    () => this.platform.listAccounts !== undefined && this.session.isAdmin(),
  );

  protected readonly account = this.session.account;
  protected readonly open = signal(false);
  protected readonly role = computed(() => {
    const role = this.account()?.role;
    return role === undefined ? '' : describeRole(role);
  });

  @HostListener('document:pointerdown', ['$event'])
  protected onPointerDown(event: Event): void {
    if (this.open() && !this.host.nativeElement.contains(event.target as globalThis.Node)) {
      this.open.set(false);
    }
  }

  @HostListener('document:keydown.escape')
  protected onEscape(): void {
    this.open.set(false);
  }

  protected toggle(): void {
    this.open.update((open) => !open);
  }

  protected changePassword(): void {
    this.open.set(false);
    this.changingPassword.emit();
  }

  protected checkForUpdates(): void {
    this.open.set(false);
    this.checkingForUpdates.emit();
  }

  protected openAppPasswords(): void {
    this.open.set(false);
    this.openingAppPasswords.emit();
  }

  protected administer(): void {
    this.open.set(false);
    void this.router.navigate(['/accounts']);
  }

  protected signOut(): void {
    this.open.set(false);
    this.signedOut.emit();
  }
}
