import type { BoardView, PrinterView } from "@/api";

export function titleCase(name: string): string {
  return name
    .split(/[_\s-]+/)
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join(" ");
}

/** The Klipper MCU name of a board in Title Case, or its USB serial when no stepper maps to it. */
export function mcuName(printer: PrinterView | null, board: BoardView): string {
  const mcu = printer?.steppers.find((s) => s.serial === board.serial)?.mcu;
  return mcu === undefined ? board.serial : titleCase(mcu);
}

const TRACE_LABEL: Record<BoardView["trace_mode"], string> = {
  unknown: "Unknown",
  dormant: "Off",
  streaming: "Streaming",
};

export function traceLabel(mode: BoardView["trace_mode"]): string {
  return TRACE_LABEL[mode];
}
