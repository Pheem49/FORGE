export interface ForgeError extends Error {
  readonly code: string;
  readonly details?: unknown;
}

export interface ForgeBridge {
  readonly runtime: "webview" | "cef";
  invoke<Res = unknown>(command: string, payload?: unknown): Promise<Res>;
  listen<Payload = unknown>(
    event: string,
    handler: (payload: Payload) => void
  ): () => void;
  once<Payload = unknown>(
    event: string,
    handler: (payload: Payload) => void
  ): () => void;
}

declare global {
  interface Window {
    readonly forge: ForgeBridge;
  }
}
