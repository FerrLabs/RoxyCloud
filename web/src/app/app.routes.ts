import type { Routes } from '@angular/router';
import { FileBrowser } from './files/file-browser';
import { PublicLink } from './public/public-link';

export const routes: Routes = [
  { path: 's/:token', component: PublicLink },
  { path: '**', component: FileBrowser },
];
