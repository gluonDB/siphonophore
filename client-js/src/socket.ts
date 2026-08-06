import { encodeWire, decodeWire } from "./wire.js";

export interface SocketOptions {
  reconnectBaseMs?: number;
  reconnectMaxMs?: number;
}

export type MessageHandler = (payload: Uint8Array) => void;
export type IncarnationHandler = (incarnation: string) => void;
export type ProtocolErrorHandler = (code: ProtocolErrorCode) => void;

export enum ProtocolErrorCode {
  InvalidMessage = 1,
  NotReady = 2,
  StaleIncarnation = 3,
  Authentication = 4,
  LoadFailed = 5,
  JoinExpired = 6,
  DocumentUnloaded = 7,
}

const MSG_INCARNATION = 2;
const INC_JOIN = 0;
const INC_INFO = 1;
const INC_READY = 2;
const INC_ERROR = 3;

function incarnationToBytes(value: string): Uint8Array {
  const hex = value.replace(/-/g, "");
  if (!/^[0-9a-fA-F]{32}$/.test(hex)) {
    throw new Error(`Invalid incarnation UUID: ${value}`);
  }
  return Uint8Array.from({ length: 16 }, (_, index) =>
    Number.parseInt(hex.slice(index * 2, index * 2 + 2), 16)
  );
}

function bytesToIncarnation(value: Uint8Array): string {
  const hex = Array.from(value, (byte) => byte.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

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
  private incarnationFns = new Map<string, Set<IncarnationHandler>>();
  private protocolErrorFns = new Map<string, Set<ProtocolErrorHandler>>();
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

  /** Begin incarnation negotiation without exposing any local CRDT state. */
  join(docId: string, previousIncarnation?: string): void {
    const previous = previousIncarnation
      ? incarnationToBytes(previousIncarnation)
      : new Uint8Array();
    const payload = new Uint8Array(2 + previous.length);
    payload.set([MSG_INCARNATION, INC_JOIN]);
    payload.set(previous, 2);
    this.send(docId, payload);
  }

  /** Confirm that the application is ready to synchronize this incarnation. */
  ready(docId: string, incarnation: string): void {
    const payload = new Uint8Array(18);
    payload.set([MSG_INCARNATION, INC_READY]);
    payload.set(incarnationToBytes(incarnation), 2);
    this.send(docId, payload);
  }

  onincarnation(docId: string, fn: IncarnationHandler): () => void {
    if (!this.incarnationFns.has(docId)) this.incarnationFns.set(docId, new Set());
    this.incarnationFns.get(docId)!.add(fn);
    return () => this.incarnationFns.get(docId)?.delete(fn);
  }

  onprotocolerror(docId: string, fn: ProtocolErrorHandler): () => void {
    if (!this.protocolErrorFns.has(docId)) this.protocolErrorFns.set(docId, new Set());
    this.protocolErrorFns.get(docId)!.add(fn);
    return () => this.protocolErrorFns.get(docId)?.delete(fn);
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
    this.incarnationFns.clear();
    this.protocolErrorFns.clear();
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
      if (payload[0] === MSG_INCARNATION && payload[1] === INC_INFO && payload.length === 18) {
        const incarnation = bytesToIncarnation(payload.slice(2));
        this.incarnationFns.get(docId)?.forEach((fn) => fn(incarnation));
        return;
      }
      if (payload[0] === MSG_INCARNATION && payload[1] === INC_ERROR && payload.length >= 3) {
        this.protocolErrorFns
          .get(docId)
          ?.forEach((fn) => fn(payload[2] as ProtocolErrorCode));
        return;
      }
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
