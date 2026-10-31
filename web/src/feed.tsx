import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  ApiRequestError,
  connectEvents,
  eventsUrl,
  getPrinter,
  postKill,
  type EventClient,
  type PrinterView,
  type ServerMessage,
} from "./api";

export type KillStatus = "idle" | "killing" | "killed" | "kill_unconfirmed";

export interface Feed {
  printer: PrinterView | null;
  connected: boolean;
  loadError: string | null;
  killStatus: KillStatus;
  killError: string | null;
  killUnconfirmed: boolean;
  kill: () => void;
  refetch: () => void;
  listen: (listener: (message: ServerMessage) => void) => () => void;
  subscribeLive: () => () => void;
}

const FeedContext = createContext<Feed | null>(null);

export function useFeed(): Feed {
  const feed = useContext(FeedContext);
  if (feed === null) throw new Error("useFeed needs a FeedProvider");
  return feed;
}

export function FeedProvider({ children }: { children: ReactNode }) {
  const [printer, setPrinter] = useState<PrinterView | null>(null);
  const [connected, setConnected] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [killStatus, setKillStatus] = useState<KillStatus>("idle");
  const [killError, setKillError] = useState<string | null>(null);
  const listeners = useRef(new Set<(message: ServerMessage) => void>());
  const client = useRef<EventClient | null>(null);
  const liveUsers = useRef(0);

  const refetch = useCallback(() => {
    getPrinter().then(
      (view) => {
        setPrinter(view);
        setLoadError(null);
      },
      (e: unknown) => setLoadError(e instanceof Error ? e.message : "request failed"),
    );
  }, []);

  useEffect(() => {
    refetch();
    const events = connectEvents({
      url: eventsUrl(window.location),
      onStatus: setConnected,
      onResync: refetch,
      onMessage: (message) => {
        if (message.type === "printer") {
          const { type: _type, ...view } = message;
          setPrinter(view);
          setLoadError(null);
        }
        listeners.current.forEach((listener) => listener(message));
      },
    });
    client.current = events;
    return () => {
      events.close();
      client.current = null;
    };
  }, [refetch]);

  const kill = useCallback(() => {
    setKillStatus("killing");
    setKillError(null);
    postKill().then(
      (result) => setKillStatus(result.state),
      (e: unknown) => {
        setKillStatus("kill_unconfirmed");
        setKillError(e instanceof ApiRequestError ? e.message : "kill request failed");
      },
    );
  }, []);

  const listen = useCallback((listener: (message: ServerMessage) => void) => {
    listeners.current.add(listener);
    return () => {
      listeners.current.delete(listener);
    };
  }, []);

  const subscribeLive = useCallback(() => {
    liveUsers.current += 1;
    if (liveUsers.current === 1) client.current?.subscribe("live");
    return () => {
      liveUsers.current -= 1;
      if (liveUsers.current === 0) client.current?.unsubscribe("live");
    };
  }, []);

  const klippy = printer?.moonraker.klippy;
  const killUnconfirmed =
    printer?.engine.unconfirmed_kill != null ||
    (killStatus === "kill_unconfirmed" && klippy !== "shutdown");

  const value = useMemo<Feed>(
    () => ({
      printer,
      connected,
      loadError,
      killStatus,
      killError,
      killUnconfirmed,
      kill,
      refetch,
      listen,
      subscribeLive,
    }),
    [printer, connected, loadError, killStatus, killError, killUnconfirmed, kill, refetch, listen, subscribeLive],
  );

  return <FeedContext.Provider value={value}>{children}</FeedContext.Provider>;
}
