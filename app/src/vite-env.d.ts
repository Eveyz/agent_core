/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_STREAMDOWN_ASSISTANT?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
