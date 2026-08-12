/// <reference types="vite/client" />

// animal-island-ui's "./style" export is plain CSS without a type declaration.
declare module "animal-island-ui/style";

// Injected by vite.config.ts from package.json during the build.
declare const __APP_VERSION__: string;
