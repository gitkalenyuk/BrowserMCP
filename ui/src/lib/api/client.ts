import type {
  ApiEvents,
  CallArgsWithOptions,
  ConnectionStatus,
  EventName,
  Method,
  Result,
} from './types';

/**
 * The only way the chrome talks to the Rust core.
 * Implementations: MockApi (in-memory, default) and WsApi (JSON-RPC over WebSocket).
 */
export interface BrowserApi {
  readonly kind: 'mock' | 'ws';
  readonly status: ConnectionStatus;
  /** Resolves once the transport is usable (mock: immediately). */
  connect(): Promise<void>;
  close(): void;
  /** `call(method, params?, options?)`: see `CallArgsWithOptions` (e.g. a longer `timeout`). */
  call<M extends Method>(method: M, ...args: CallArgsWithOptions<M>): Promise<Result<M>>;
  on<E extends EventName>(event: E, handler: (payload: ApiEvents[E]) => void): () => void;
  onStatus(handler: (status: ConnectionStatus) => void): () => void;
}

export class RpcError extends Error {
  constructor(
    readonly code: string,
    message: string,
    readonly data?: unknown,
  ) {
    super(message);
    this.name = 'RpcError';
  }
}

/** The core did not answer in time (`CallOptions.timeout`): the command may still be running. */
export const isTimeout = (err: unknown): boolean => err instanceof RpcError && err.code === 'TIMEOUT';

/** A usable per-call timeout (positive, finite), else `fallback`. */
export function timeoutOf(options: { timeout?: number } | undefined, fallback: number): number {
  const t = options?.timeout;
  // setTimeout fires at once for delays above 2^31 - 1 ms.
  return typeof t === 'number' && Number.isFinite(t) && t > 0 ? Math.min(t, 2_147_483_647) : fallback;
}

type AnyHandler = (payload: never) => void;

/** Tiny typed event emitter shared by the implementations. */
export class Emitter {
  #handlers = new Map<string, Set<AnyHandler>>();

  on<E extends EventName>(event: E, handler: (payload: ApiEvents[E]) => void): () => void {
    let set = this.#handlers.get(event);
    if (!set) this.#handlers.set(event, (set = new Set()));
    set.add(handler as AnyHandler);
    return () => set.delete(handler as AnyHandler);
  }

  emit<E extends EventName>(event: E, payload: ApiEvents[E]): void {
    const set = this.#handlers.get(event);
    if (!set) return;
    for (const h of set) {
      try {
        (h as (p: ApiEvents[E]) => void)(payload);
      } catch (err) {
        console.error(`[api] handler for ${event} failed`, err);
      }
    }
  }

  /** Emit for an event name received off the wire (unknown names are ignored by having no handlers). */
  emitRaw(event: string, payload: unknown): void {
    this.emit(event as EventName, payload as never);
  }
}
