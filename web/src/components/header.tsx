import { useConfirmClick } from "@/confirm";
import { useFeed } from "@/feed";
import { cn } from "@/lib/utils";
import { Button } from "./ui";

export function Header() {
  const { printer, connected, killStatus, killUnconfirmed, killError, kill } = useFeed();
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
      <header className="sticky top-0 z-50 flex h-(--header-height) items-center gap-x-6 border-b bg-background px-4 text-sm">
        <span className="font-medium tracking-wide">foci-tune</span>
        <span className={cn("font-mono", klippyClass)}>Klipper {klippy}</span>
        {!connected && <span className="text-amber-400">Event stream disconnected</span>}
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
          The emergency stop is not confirmed and motion may continue. Press Kill again to re-post the stop.
          {killError !== null && <span className="ml-2 font-mono">{killError}</span>}
        </div>
      )}
    </>
  );
}
