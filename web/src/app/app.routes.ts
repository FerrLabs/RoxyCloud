import { inject } from '@angular/core';
import { Router, type Routes } from '@angular/router';
import { Accounts } from './accounts/accounts';
import { FileBrowser } from './files/file-browser';
import { FILES } from './folder';
import { PublicLink } from './public/public-link';
import { SyncView } from './sync/sync-view';

export const routes: Routes = [
  { path: 's/:token', component: PublicLink },
  { path: 'accounts', component: Accounts },
  { path: 'sync', component: SyncView },
  { path: FILES, children: [{ path: '**', component: FileBrowser }] },
  {
    path: '**',
    redirectTo: ({ url }) =>
      inject(Router).createUrlTree(['/', FILES, ...url.map((segment) => segment.path)]),
  },
];
