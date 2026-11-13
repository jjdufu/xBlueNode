/// <reference types="vite/client" />

/** Application version injected by Vite from package.json at build time. */
declare const __APP_VERSION__: string;

interface ImportMetaEnv {
  readonly VITE_DEV_TOKEN?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
