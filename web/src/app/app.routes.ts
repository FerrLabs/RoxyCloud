import type { Routes } from '@angular/router';
import { Accounts } from './accounts/accounts';
import { FileBrowser } from './files/file-browser';
import { PublicLink } from './public/public-link';
import { SyncView } from './sync/sync-view';

export const routes: Routes = [
  { path: 's/:token', component: PublicLink },
  { path: 'accounts', component: Accounts },
  { path: 'sync', component: SyncView },
  { path: '**', component: FileBrowser },
];
