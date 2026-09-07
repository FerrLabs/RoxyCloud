import { Injectable } from '@angular/core';
import { encodePath } from '../platform';
import type { Linked } from '../share';

export class LinkNeedsPassword extends Error {
  constructor() {
    super('this link needs its password');
  }
}

export class LinkIsGone extends Error {
  constructor() {
    super('this link is no longer available');
  }
}

export class LinkIsBusy extends Error {
  readonly seconds: number;

  constructor(seconds: number) {
    super('too many attempts on this link');
    this.seconds = seconds;
  }
}

@Injectable({ providedIn: 'root' })
export class PublicLinks {
  async open(token: string, password: string, path: string): Promise<Linked> {
    const below = encodePath(path);
    const at = below.length === 0 ? '' : `/entries${below}`;
    const response = await this.get(token, at, password);
    return (await response.json()) as Linked;
  }

  async download(token: string, password: string, path: string, name: string): Promise<void> {
    const response = await this.get(token, `/content${encodePath(path)}`, password);
    const href = URL.createObjectURL(await response.blob());
    const link = document.createElement('a');
    link.href = href;
    link.download = name;
    link.click();
    URL.revokeObjectURL(href);
  }

  private async get(token: string, at: string, password: string): Promise<Response> {
    const response = await fetch(
      `${ROXYCLOUD_API_URL}/v1/public/${encodeURIComponent(token)}${at}`,
      {
        headers: password.length > 0 ? { 'X-Share-Password': password } : {},
      },
    );

    if (response.status === 401) {
      throw new LinkNeedsPassword();
    }
    if (response.status === 429) {
      throw new LinkIsBusy(Number(response.headers.get('Retry-After') ?? 60));
    }
    if (!response.ok) {
      throw new LinkIsGone();
    }
    return response;
  }
}
