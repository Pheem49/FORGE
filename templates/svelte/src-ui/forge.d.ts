export interface ForgeCommandTypes {}

export interface ForgeError extends Error {
  readonly code: string;
  readonly details?: unknown;
}

export interface ForgeCreateWindowOptions {
  label: string;
  url?: string;
  title?: string;
  width?: number;
  height?: number;
  x?: number;
  y?: number;
  resizable?: boolean;
  decorations?: boolean;
  visible?: boolean;
}

export interface ForgeWindowBridge {
  readonly label: string;
  close(): Promise<void>;
  minimize(minimized?: boolean): Promise<void>;
  maximize(maximized?: boolean): Promise<void>;
  toggleFullscreen(fullscreen?: boolean): Promise<void>;
  setTitle(title: string): Promise<void>;
  show(): Promise<void>;
  hide(): Promise<void>;
  focus(): Promise<void>;
  startDragging(): Promise<void>;
  create(opts: ForgeCreateWindowOptions): Promise<string>;
  list(): Promise<string[]>;
}

export interface ForgeBridge {
  readonly runtime: "webview" | "cef";
  readonly window: ForgeWindowBridge;
  invoke<Res = unknown, N extends string = string>(
    command: N,
    ...args: N extends keyof ForgeCommandTypes
      ? [payload: ForgeCommandTypes[N]["payload"]]
      : [payload?: unknown]
  ): Promise<
    N extends keyof ForgeCommandTypes ? ForgeCommandTypes[N]["result"] : Res
  >;
  listen<Payload = unknown>(
    event: string,
    handler: (payload: Payload) => void
  ): () => void;
  once<Payload = unknown>(
    event: string,
    handler: (payload: Payload) => void
  ): () => void;
  emit(event: string, payload?: unknown): Promise<void>;
  emitTo(target: string, event: string, payload?: unknown): Promise<void>;
}

declare global {
  interface Window {
    readonly forge: ForgeBridge;
  }
}
