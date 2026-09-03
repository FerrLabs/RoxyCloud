import { type ApplicationConfig, provideZonelessChangeDetection } from '@angular/core';
import { providePlatform } from './platform';

export const appConfig: ApplicationConfig = {
  providers: [provideZonelessChangeDetection(), providePlatform()],
};
