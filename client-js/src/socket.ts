import { encodeWire, decodeWire } from "./wire.js";

export interface SocketOptions {
  reconnectBaseMs?: number;
  reconnectMaxMs?: number;
}

export type MessageHandler = (payload: Uint8Array) => void;

/**
 * Multiplexed WebSocket for Siphonophore's wire protocol.
 *
 * Routes raw binary payloads by document ID. Handles reconnection with
 * exponential backoff. Has zero Yjs dependencies -- bring your own
 * y-protocols / lib0 / awareness on top.
 */
export class SiphonophoreSocket {
  readonly url: string;

  private ws: WebSocket | null = null;
  private subs = new Map<string, Set<MessageHandler>>();
  private connectFns = new Set<() => void>();
  private disconnectFns = new Set<() => void>();
  private destroyed = false;
  private reconnectAttempt = 0;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;

  private readonly reconnectBaseMs: number;
  private readonly reconnectMaxMs: number;

  constructor(url: string, opts?: SocketOptions) {
    this.url = url;
    this.reconnectBaseMs = opts?.reconnectBaseMs ?? 500;
    this.reconnectMaxMs = opts?.reconnectMaxMs ?? 30_000;
    this.open();
  }

  get connected(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  /** Subscribe to raw payloads for a given docId. Returns an unsubscribe fn. */
  subscribe(docId: string, handler: MessageHandler): () => void {
    if (!this.subs.has(docId)) this.subs.set(docId, new Set());
    this.subs.get(docId)!.add(handler);
    return () => {
      this.subs.get(docId)?.delete(handler);
    };
  }

  /** Send a raw payload for a given docId (wire-framed automatically). */
  send(docId: string, payload: Uint8Array): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(encodeWire(docId, payload));
    }
  }

  /** Register a callback fired every time the WebSocket (re)opens. */
  onconnect(fn: () => void): () => void {
    this.connectFns.add(fn);
    return () => {
      this.connectFns.delete(fn);
    };
  }

  /** Ask the server to persist a document's current state. */
  save(docId: string): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify({ action: "save", doc: docId }));
    }
  }

  /** Tell the server this client is leaving a document. */
  leave(docId: string): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify({ action: "leave", doc: docId }));
    }
  }

  /** Register a callback fired every time the WebSocket closes. */
  ondisconnect(fn: () => void): () => void {
    this.disconnectFns.add(fn);
    return () => {
      this.disconnectFns.delete(fn);
    };
  }

  destroy(): void {
    this.destroyed = true;
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.subs.clear();
    this.connectFns.clear();
    this.disconnectFns.clear();
    if (this.ws) {
      this.ws.onclose = null;
      this.ws.close();
      this.ws = null;
    }
  }

  private open(): void {
    if (this.destroyed) return;
    const ws = new WebSocket(this.url);
    ws.binaryType = "arraybuffer";
    this.ws = ws;

    ws.onopen = () => {
      this.reconnectAttempt = 0;
      this.connectFns.forEach((fn) => fn());
    };

    ws.onmessage = (event: MessageEvent) => {
      const data = new Uint8Array(event.data as ArrayBuffer);
      const { docId, payload } = decodeWire(data);
      const fns = this.subs.get(docId);
      if (fns) fns.forEach((fn) => fn(payload));
    };

    ws.onclose = () => {
      if (this.destroyed) return;
      this.disconnectFns.forEach((fn) => fn());
      this.scheduleReconnect();
    };

    ws.onerror = () => {
      ws.close();
    };
  }

  private scheduleReconnect(): void {
    if (this.destroyed) return;
    const delay = Math.min(
      this.reconnectBaseMs * Math.pow(2, this.reconnectAttempt),
      this.reconnectMaxMs
    );
    this.reconnectAttempt++;
    this.reconnectTimer = setTimeout(() => this.open(), delay);
  }
}
