/// <reference types="svelte" />
/// <reference types="vite/client" />

// vite.config.ts の define で置換されるビルド指紋（git short sha·HHmm、失敗時 "dev"）。
declare const __BUILD_ID__: string;
