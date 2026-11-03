import { mdiCarBrakeAlert, mdiHome } from "@mdi/js";
import { useEffect, useState } from "react";
import { ApiRequestError, postHome } from "@/api";
import { useConfirmClick } from "@/confirm";
import { useFeed } from "@/feed";
import { cn } from "@/lib/utils";

export function homedTone(axes: string): "full" | "partial" | "none" {
  if (axes === "") return "none";
  return ["x", "y", "z"].every((a) => axes.includes(a)) ? "full" : "partial";
}

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
  const idle = killLabel === "Kill";
  const homedAxes = printer?.moonraker.homed_axes ?? "";
  const homedClass = {
    full: "text-emerald-400",
    partial: "text-amber-400",
    none: "",
  }[homedTone(homedAxes)];

  const [homing, setHoming] = useState(false);
  const [homeError, setHomeError] = useState<string | null>(null);
  useEffect(() => setHomeError(null), [klippy]);
  const home = () => {
    setHoming(true);
    setHomeError(null);
    postHome().then(
      () => setHoming(false),
      (e: unknown) => {
        setHoming(false);
        setHomeError(e instanceof ApiRequestError ? e.message : "request failed");
      },
    );
  };

  return (
    <>
      <header className="sticky top-0 z-50 flex h-(--header-height) items-center gap-x-6 border-b bg-background px-4 text-sm">
        <span className={cn("font-mono", klippyClass)}>Klipper {klippy}</span>
        {!connected && <span className="text-amber-400">Event stream disconnected</span>}
        <div className="ml-auto flex items-center gap-3">
          {homeError !== null && <span className="text-amber-400">{homeError}</span>}
          {killStatus === "killed" && !killUnconfirmed && (
            <span className="font-mono text-red-400">Stop confirmed</span>
          )}
          <button
            type="button"
            aria-label={killLabel}
            title="Emergency stop"
            onClick={onKill}
            className={cn(
              "flex h-9 items-center gap-2 rounded-md border px-2 text-sm font-medium transition-colors focus-visible:outline-2",
              idle ? "text-red-400 hover:bg-accent" : "border-red-500 bg-red-600 text-white hover:bg-red-500",
            )}
          >
            <svg viewBox="0 0 24 24" aria-hidden="true" className="size-6 fill-current">
              <path d={mdiCarBrakeAlert} />
            </svg>
            {!idle && <span>{killLabel}</span>}
          </button>
          <button
            type="button"
            aria-label={homing ? "Homing" : "Home"}
            title={`Home all axes (homed: ${homedAxes === "" ? "none" : homedAxes})`}
            disabled={homing || klippy !== "ready"}
            onClick={home}
            className={cn(
              "flex h-9 items-center rounded-md border px-2 transition-colors hover:bg-accent focus-visible:outline-2 disabled:cursor-not-allowed disabled:opacity-50",
              homedClass,
            )}
          >
            <svg viewBox="0 0 24 24" aria-hidden="true" className="size-6 fill-current">
              <path d={mdiHome} />
            </svg>
          </button>
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
