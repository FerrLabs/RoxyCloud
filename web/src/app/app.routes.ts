import type { Routes } from '@angular/router';
import { FileBrowser } from './files/file-browser';

export const routes: Routes = [{ path: '**', component: FileBrowser }];
