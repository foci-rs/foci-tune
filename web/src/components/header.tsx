import { Link } from "@tanstack/react-router";
import type { PrinterView } from "@/api";
import { useConfirmClick } from "@/confirm";
import { useFeed } from "@/feed";
import { formatBytes } from "@/units";
import { cn } from "@/lib/utils";
import { Button, Dot } from "./ui";

export function traceTone(printer: PrinterView | null): "ok" | "warn" | "off" {
  if (printer === null || printer.boards.length === 0) return "off";
  return printer.boards.every((b) => b.open) ? "ok" : "warn";
}

const navClass =
  "rounded-full px-3 py-1 text-sm text-muted-foreground hover:text-foreground";

export function Header() {
  const feed = useFeed();
  const { printer, connected, killStatus, killUnconfirmed, killError, kill } = feed;
  const klippy = printer?.moonraker.klippy ?? "disconnected";
  const klippyClass =
    klippy === "ready"
      ? "text-emerald-400"
      : klippy === "shutdown" || klippy === "error"
        ? "text-red-400"
        : "text-muted-foreground";

  const confirm = useConfirmClick(kill);
  const onKill = killUnconfirmed ? kill : confirm.click;
  const killLabel =
    killStatus === "killing"
      ? "Killing"
      : killUnconfirmed
        ? "Kill again"
        : confirm.armed
          ? "Press again to kill"
          : "Kill";

  return (
    <>
      <header className="flex flex-wrap items-center gap-x-6 gap-y-2 border-b px-4 py-3 text-sm">
        <span className="font-medium tracking-wide">foci-tune</span>
        <nav aria-label="Pages" className="flex gap-1">
          <Link to="/printer" className={navClass} activeProps={{ className: "bg-accent text-foreground" }}>
            Printer
          </Link>
          <Link to="/tune" className={navClass} activeProps={{ className: "bg-accent text-foreground" }}>
            Tune
          </Link>
          <Link to="/sessions" className={navClass} activeProps={{ className: "bg-accent text-foreground" }}>
            Sessions
          </Link>
        </nav>
        <ul className="flex flex-wrap items-center gap-x-4 gap-y-1">
          <li className="flex items-center gap-2">
            <Dot tone={printer?.moonraker.connected ? "ok" : "off"} />
            Moonraker
            <span className="sr-only">{printer?.moonraker.connected ? "connected" : "not connected"}</span>
          </li>
          <li className="flex items-center gap-2">
            <Dot tone={traceTone(printer)} />
            Trace
            <span className="sr-only">{traceTone(printer) === "ok" ? "open" : "not open"}</span>
          </li>
          <li className={cn("font-mono", klippyClass)}>Klipper {klippy}</li>
          <li className="font-mono text-muted-foreground">
            Homed {printer && printer.moonraker.homed_axes !== "" ? printer.moonraker.homed_axes : "none"}
          </li>
          <li className="font-mono text-muted-foreground">
            Free {printer?.free_bytes != null ? formatBytes(printer.free_bytes) : "unknown"}
          </li>
          {!connected && <li className="text-amber-400">Event stream disconnected</li>}
        </ul>
        <div className="ml-auto flex items-center gap-3">
          {killStatus === "killed" && !killUnconfirmed && (
            <span className="font-mono text-red-400">Stop confirmed</span>
          )}
          <Button tone="danger" onClick={onKill} className="min-w-40">
            {killLabel}
          </Button>
        </div>
      </header>
      {killUnconfirmed && (
        <div role="alert" className="border-b border-red-500 bg-red-950 px-4 py-2 text-sm text-red-200">
          The emergency stop is not confirmed and motion may continue. Press Kill again to re-post
          the stop.
          {killError !== null && <span className="ml-2 font-mono">{killError}</span>}
        </div>
      )}
    </>
  );
}
