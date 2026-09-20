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
  private readonly session = inject(Session);
  private readonly platform = inject(PLATFORM);

  readonly changingPassword = output<void>();
  readonly openingAppPasswords = output<void>();
  readonly signedOut = output<void>();

  protected readonly canMintAppPasswords = this.platform.mintAppPassword !== undefined;

  protected readonly account = this.session.account;
  protected readonly open = signal(false);
  protected readonly canChangePassword = this.platform.changePassword !== undefined;
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

  protected openAppPasswords(): void {
    this.open.set(false);
    this.openingAppPasswords.emit();
  }

  protected signOut(): void {
    this.open.set(false);
    this.signedOut.emit();
  }
}
