import type { BoardView, PrinterView } from "@/api";
import { mcuName, traceLabel } from "@/mcu";
import { Card, Field } from "./ui";

export function McuCard({ printer, board }: { printer: PrinterView; board: BoardView }) {
  const streaming = board.trace_mode === "streaming";
  const channels = board.channels.length;
  return (
    <Card label={`${mcuName(printer, board)} (${channels} ${channels === 1 ? "Channel" : "Channels"})`}>
      {!board.open && <p className="mb-3 text-sm text-amber-400">Closed</p>}
      <dl className="grid grid-cols-2 gap-3">
        <div className="col-span-2 break-all">
          <Field label="Chip ID">{board.serial}</Field>
        </div>
        <Field label="Board">{board.product ?? "-"}</Field>
        <Field label="Firmware">
          {board.mcu_version == null ? "-" : <span title={board.mcu_build ?? undefined}>{board.mcu_version}</span>}
        </Field>
        <Field label="Trace">{traceLabel(board.trace_mode)}</Field>
        <Field label="Sampling frequency">{streaming ? `${board.critical_hz_measured.toFixed(0)} Hz` : "-"}</Field>
      </dl>
      {board.unusable_reason !== null && <p className="mt-3 text-sm text-amber-400">{board.unusable_reason}</p>}
    </Card>
  );
}
