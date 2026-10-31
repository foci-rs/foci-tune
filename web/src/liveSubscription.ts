import type { EventClient } from "./api";

export interface LiveSubscriptions {
  acquire(): () => void;
  attach(client: EventClient | null): void;
}

export function createLiveSubscriptions(): LiveSubscriptions {
  let users = 0;
  let client: EventClient | null = null;

  return {
    acquire() {
      users += 1;
      if (users === 1) client?.subscribe("live");
      let released = false;
      return () => {
        if (released) return;
        released = true;
        users -= 1;
        if (users === 0) client?.unsubscribe("live");
      };
    },
    attach(next) {
      client = next;
      if (next !== null && users > 0) next.subscribe("live");
    },
  };
}
