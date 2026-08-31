// WebSocket client. Messages are JSON objects tagged by `type`;
// crates/core/src/protocol.rs is the source of truth.

export class Net {
  constructor() {
    this.ws = null;
    this.handlers = {};
  }

  on(type, fn) {
    this.handlers[type] = fn;
    return this;
  }

  connect() {
    return new Promise((resolve, reject) => {
      const proto = location.protocol === 'https:' ? 'wss' : 'ws';
      this.ws = new WebSocket(`${proto}://${location.host}/ws`);
      this.ws.onopen = () => resolve();
      this.ws.onerror = () => reject(new Error('cannot reach the game server'));
      this.ws.onclose = () => this.handlers.close?.();
      this.ws.onmessage = (ev) => {
        let msg;
        try { msg = JSON.parse(ev.data); } catch { return; }
        this.handlers[msg.type]?.(msg);
      };
    });
  }

  send(obj) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(obj));
    }
  }
}
