import { type ApplicationConfig, mergeApplicationConfig } from '@angular/core';
import { provideServerRendering, withRoutes } from '@angular/ssr';
import { appConfig } from './app.config';
import { serverRoutes } from './app.routes.server';

const server: ApplicationConfig = {
  providers: [provideServerRendering(withRoutes(serverRoutes))],
};

export const serverConfig = mergeApplicationConfig(appConfig, server);
