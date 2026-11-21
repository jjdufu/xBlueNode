/// <reference types="vite/client" />

/** Application version injected by Vite from package.json at build time. */
declare const __APP_VERSION__: string;

interface ImportMetaEnv {
  readonly VITE_DEV_TOKEN?: string;
  readonly AXPOINT_MAP_TILE_URL?: string;
  readonly VITE_MAP_TILE_URL?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
