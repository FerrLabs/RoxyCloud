import type { Routes } from '@angular/router';
import type { Locale } from './content/locale';
import { Api } from './pages/api/api';
import { Home } from './pages/home/home';
import { Install } from './pages/install/install';

const pages = (locale: Locale): Routes => [
  { path: '', component: Home, data: { locale } },
  { path: 'install', component: Install, data: { locale } },
  { path: 'api', component: Api, data: { locale } },
];

export const routes: Routes = [...pages('en'), { path: 'fr', children: pages('fr') }];
