import { mdiCarBrakeAlert, mdiHome } from "@mdi/js";
import { Link } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { ApiRequestError, postHome, type PrinterView } from "@/api";
import { CONFIRM_WINDOW_MS, useConfirmClick } from "@/confirm";
import { useFeed } from "@/feed";
import { cn } from "@/lib/utils";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { SidebarMenu, SidebarMenuButton, SidebarMenuItem } from "@/components/ui/sidebar";
import { mcuName, traceLabel } from "@/mcu";
import { Dot } from "./ui";

export function homedTone(axes: string): "full" | "partial" | "none" {
  if (axes === "") return "none";
  return ["x", "y", "z"].every((a) => axes.includes(a)) ? "full" : "partial";
}

type Tone = "ok" | "warn" | "error" | "off";

export function connectionStatuses(printer: PrinterView | null): {
  moonraker: Tone;
  klipper: Tone;
  foci: Tone;
  tracing: Tone;
} {
  const boards = printer?.boards ?? [];
  const klippy = printer?.moonraker.klippy ?? "disconnected";
  const all = (n: number) => (n === 0 ? "off" : n === boards.length ? "ok" : "warn");
  return {
    moonraker: printer?.moonraker.connected ? "ok" : "off",
    klipper:
      klippy === "ready"
        ? "ok"
        : klippy === "startup"
          ? "warn"
          : klippy === "shutdown" || klippy === "error"
            ? "error"
            : "off",
    foci: boards.length === 0 ? "off" : all(boards.filter((b) => b.open).length),
    tracing: boards.length === 0 ? "off" : all(boards.filter((b) => b.open && b.trace_mode === "streaming").length),
  };
}

export function connectionDetails(
  printer: PrinterView | null,
): Record<"moonraker" | "klipper" | "foci" | "tracing", string[]> {
  const boards = printer?.boards ?? [];
  const name = (b: PrinterView["boards"][number]) => mcuName(printer, b);
  const none = ["No boards found"];
  return {
    moonraker: [printer?.moonraker.connected ? "Connected to Moonraker" : "Not connected to Moonraker"],
    klipper: printer
      ? [
          `Klippy: ${printer.moonraker.klippy}`,
          `Print state: ${printer.moonraker.print_state}`,
          `Idle state: ${printer.moonraker.idle_state}`,
        ]
      : ["Klippy: disconnected"],
    foci: boards.length === 0 ? none : boards.map((b) => `${name(b)}: ${b.open ? "open" : "closed"}`),
    tracing: boards.length === 0 ? none : boards.map((b) => `${name(b)}: ${traceLabel(b.trace_mode)}`),
  };
}

function StatusItem({ label, state, tone, details }: { label: string; state: string; tone: Tone; details: string[] }) {
  return (
    <li>
      <Tooltip>
        <TooltipTrigger asChild>
          <div tabIndex={0} className="flex items-center gap-2">
            <Dot tone={tone} />
            <span aria-hidden="true">{label}</span>
            <span className="sr-only">
              {label} {state}
            </span>
          </div>
        </TooltipTrigger>
        <TooltipContent side="bottom" align="start">
          {details.map((line) => (
            <p key={line}>{line}</p>
          ))}
        </TooltipContent>
      </Tooltip>
    </li>
  );
}

export function Header() {
  const { printer, connected, killStatus, killUnconfirmed, killError, kill } = useFeed();
  const klippy = printer?.moonraker.klippy ?? "disconnected";
  const tones = connectionStatuses(printer);
  const boards = printer?.boards ?? [];
  const details = connectionDetails(printer);

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
  // biome-ignore lint/correctness/useExhaustiveDependencies: klippy is a trigger: a new Klippy state clears the home error.
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
      <header className="sticky top-0 z-50 flex h-(--header-height) items-center border-b bg-background text-sm">
        <div className="flex h-full w-(--sidebar-width) shrink-0 items-center border-r p-2">
          <SidebarMenu>
            <SidebarMenuItem>
              <SidebarMenuButton size="lg" asChild>
                <Link to="/printer">
                  <img src="/foci-icon.svg" alt="" className="size-8 rounded-lg" />
                  <div className="grid flex-1 text-left text-sm leading-tight">
                    <span className="truncate font-medium">FOCI</span>
                    <span className="truncate text-xs text-muted-foreground">Motion Tuning</span>
                  </div>
                </Link>
              </SidebarMenuButton>
            </SidebarMenuItem>
          </SidebarMenu>
        </div>
        <div className="flex flex-1 items-center gap-x-6 px-4">
          <ul aria-label="Connections" className="flex items-center gap-x-4">
            <StatusItem
              label="Moonraker"
              tone={tones.moonraker}
              state={printer?.moonraker.connected ? "connected" : "not connected"}
              details={details.moonraker}
            />
            <StatusItem label="Klipper" tone={tones.klipper} state={klippy} details={details.klipper} />
            <StatusItem
              label="FOCI"
              tone={tones.foci}
              state={`${boards.filter((b) => b.open).length} of ${boards.length} boards open`}
              details={details.foci}
            />
            <StatusItem
              label="Tracing"
              tone={tones.tracing}
              state={`${boards.filter((b) => b.open && b.trace_mode === "streaming").length} of ${boards.length} boards streaming`}
              details={details.tracing}
            />
          </ul>
          {!connected && <span className="text-amber-400">Event stream disconnected</span>}
          <div className="ml-auto flex items-center gap-3">
            {homeError !== null && <span className="text-amber-400">{homeError}</span>}
            {killStatus === "killed" && !killUnconfirmed && (
              <span className="font-mono text-red-400">Stop confirmed</span>
            )}
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  type="button"
                  aria-label={killLabel}
                  aria-pressed={!idle}
                  onClick={onKill}
                  className={cn(
                    "flex h-9 items-center rounded-md border px-2 transition-colors focus-visible:outline-2",
                    idle ? "text-red-400 hover:bg-accent" : "border-red-500 bg-red-600 text-white hover:bg-red-500",
                  )}
                >
                  <svg viewBox="0 0 24 24" aria-hidden="true" className="size-6 fill-current">
                    <path d={mdiCarBrakeAlert} />
                  </svg>
                </button>
              </TooltipTrigger>
              <TooltipContent side="bottom" align="end">
                {idle
                  ? "Emergency stop. Click twice to confirm."
                  : `Click again within ${CONFIRM_WINDOW_MS / 1000} seconds to stop.`}
              </TooltipContent>
            </Tooltip>
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
