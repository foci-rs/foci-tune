import { useState } from "react";
import {
  ApiRequestError,
  postFirmwareRestart,
  postTraceMode,
  type BoardView,
  type ChannelView,
  type PrinterView,
  type StepperView,
  type TraceModeCommand,
} from "@/api";
import { cn } from "@/lib/utils";
import { cornerHz, formatHz, pGain, quantumUm } from "@/units";
import { Button, Card, Field } from "./ui";

function dash<T>(value: T | null, render: (v: T) => string): string {
  return value === null ? "-" : render(value);
}

export function StepperTable({ steppers }: { steppers: PrinterView["steppers"] }) {
  if (steppers.length === 0) {
    return <p className="text-sm text-muted-foreground">No FOCI steppers found.</p>;
  }
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-sm">
        <thead className="text-xs uppercase tracking-wider text-muted-foreground">
          <tr>
            <th className="py-1 pr-4 font-medium">Stepper</th>
            <th className="py-1 pr-4 font-medium">Board</th>
            <th className="py-1 pr-4 font-medium">Channel</th>
            <th className="py-1 pr-4 font-medium">Axis</th>
            <th className="py-1 pr-4 font-medium">Travel mm</th>
            <th className="py-1 pr-4 font-medium">Rotation mm</th>
            <th className="py-1 pr-4 font-medium">Encoder ppr</th>
            <th className="py-1 pr-4 font-medium">Electrical model</th>
            <th className="py-1 pr-4 font-medium">Config vel P / I</th>
            <th className="py-1 pr-4 font-medium">Pattern</th>
            <th className="py-1 font-medium">Status</th>
          </tr>
        </thead>
        <tbody className="font-mono">
          {steppers.map((s) => (
            <tr key={s.name} className="border-t align-top">
              <td className="py-1.5 pr-4">{s.name}</td>
              <td className="py-1.5 pr-4">{s.serial}</td>
              <td className="py-1.5 pr-4">{s.channel}</td>
              <td className="py-1.5 pr-4">{s.axis}</td>
              <td className="py-1.5 pr-4">
                {s.position_min} to {s.position_max}
              </td>
              <td className="py-1.5 pr-4">{s.motor_rotation_distance_mm}</td>
              <td className="py-1.5 pr-4">{s.encoder_ppr}</td>
              <td className="py-1.5 pr-4">{s.electrical_model_present ? "present" : "missing"}</td>
              <td className="py-1.5 pr-4">
                {dash(s.config_gains.velocity_p, String)} / {dash(s.config_gains.velocity_i, String)}
              </td>
              <td className="py-1.5 pr-4">
                {s.proposal.pattern !== null
                  ? `${s.proposal.pattern.start.join(",")} to ${s.proposal.pattern.end.join(",")}`
                  : (s.proposal.refusal ?? "-")}
              </td>
              <td className="py-1.5 font-sans">
                {s.unusable !== null ? (
                  <span className="text-amber-400">{s.unusable}</span>
                ) : (
                  <span className="text-emerald-400">usable</span>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function ControlStateCard({
  stepper,
  board,
  channel,
}: {
  stepper: StepperView | undefined;
  board: BoardView;
  channel: ChannelView;
}) {
  const c = channel.control;
  const corner = channel.velocity_i_corner_hz ?? cornerHz(c.velocity_i, c.pwm_hz);
  const title = `${stepper?.name ?? "unmapped"} (${board.serial} channel ${channel.channel})`;
  return (
    <Card label={title}>
      <dl className="grid grid-cols-2 gap-x-4 gap-y-3 sm:grid-cols-3">
        <Field label="Velocity P" sub={`${pGain(c.velocity_p)}`}>
          {c.velocity_p}
        </Field>
        <Field label="Velocity I" sub={`corner ${formatHz(corner, 2)}`}>
          {c.velocity_i}
        </Field>
        <Field label="Position P" sub={`${pGain(c.position_p)}`}>
          {c.position_p}
        </Field>
        <Field label="Position I">{c.position_i}</Field>
        <Field label="Torque P / I">
          {c.torque_p} / {c.torque_i}
        </Field>
        <Field label="Flux P / I">
          {c.flux_p} / {c.flux_i}
        </Field>
        <Field label="Velocity filter">{c.velocity_filter_hz} Hz</Field>
        <Field label="Torque filter">{c.torque_filter_hz} Hz</Field>
        <Field label="Position filter">{c.position_filter_hz} Hz</Field>
        <Field label="Flux filter">{c.flux_filter_hz} Hz</Field>
        <Field
          label="Velocity feedforward"
          sub={c.velocity_ff_enable ? `${(c.velocity_ff_permille / 10).toFixed(1)} %` : "off"}
        >
          {c.velocity_ff_permille}
        </Field>
        <Field label="Accel / decel feedforward" sub={c.accel_ff_enable ? "on" : "off"}>
          {c.accel_ff_gain} / {c.decel_ff_gain}
        </Field>
        <Field label="Run current">{c.run_current}</Field>
        <Field label="Voltage limit">{c.voltage_limit}</Field>
        <Field label="PWM">{c.pwm_hz} Hz</Field>
        <Field
          label="Resolution"
          sub={
            channel.quantum_units !== null && stepper !== undefined
              ? `${channel.quantum_units} units, ${quantumUm(channel.quantum_units, stepper.motor_rotation_distance_mm).toFixed(2)} um`
              : undefined
          }
        >
          {c.encoder_counts_per_rev} counts/rev
        </Field>
        <Field label="Change sequence">{c.change_sequence}</Field>
        <Field label="Last applied request" sub={`tick ${c.applied_tick}`}>
          code {c.applied_request}
        </Field>
      </dl>
    </Card>
  );
}

const MODES: TraceModeCommand[] = ["dormant", "critical", "diagnostic"];

export function traceModeBlocked(printer: PrinterView, board: BoardView): string | null {
  if (!board.open) return "The trace interface is not open.";
  if (board.unusable_reason !== null) return board.unusable_reason;
  if (printer.moonraker.klippy !== "ready") return `Klipper is ${printer.moonraker.klippy}.`;
  if (printer.engine.unconfirmed_kill !== null) {
    return "The last emergency stop is unconfirmed.";
  }
  if (printer.engine.current !== null || printer.engine.queue.length > 0) {
    return "A run is in flight or queued.";
  }
  return null;
}

export function TraceModeCard({ printer, board }: { printer: PrinterView; board: BoardView }) {
  const [pending, setPending] = useState<TraceModeCommand | null>(null);
  const [error, setError] = useState<string | null>(null);
  const blocked = traceModeBlocked(printer, board);

  const choose = (mode: TraceModeCommand) => {
    setPending(mode);
    setError(null);
    postTraceMode(board.serial, mode).then(
      () => setPending(null),
      (e: unknown) => {
        setPending(null);
        setError(e instanceof ApiRequestError ? e.message : "request failed");
      },
    );
  };

  return (
    <Card label={`Trace mode ${board.serial}`}>
      <dl className="mb-3 grid grid-cols-2 gap-3 sm:grid-cols-4">
        <Field label="Current">{board.trace_mode}</Field>
        <Field label="Critical cadence">{board.critical_hz_measured.toFixed(0)} Hz</Field>
        <Field label="Protocol">{dash(board.protocol_version, String)}</Field>
        <Field label="Firmware">{board.firmware_id ?? "-"}</Field>
      </dl>
      <div role="group" aria-label={`Trace mode for ${board.serial}`} className="flex gap-2">
        {MODES.map((mode) => (
          <Button
            key={mode}
            aria-pressed={board.trace_mode === mode}
            disabled={blocked !== null || pending !== null}
            onClick={() => choose(mode)}
            className={cn(board.trace_mode === mode && "border-emerald-500")}
          >
            {mode}
          </Button>
        ))}
      </div>
      {blocked !== null && <p className="mt-2 text-sm text-muted-foreground">{blocked}</p>}
      {error !== null && (
        <p role="alert" className="mt-2 text-sm text-amber-400">
          {error}
        </p>
      )}
    </Card>
  );
}

export function FirmwareRestart() {
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const restart = () => {
    setBusy(true);
    setMessage(null);
    postFirmwareRestart().then(
      () => {
        setBusy(false);
        setMessage("Firmware restart requested.");
      },
      (e: unknown) => {
        setBusy(false);
        setMessage(e instanceof ApiRequestError ? e.message : "request failed");
      },
    );
  };
  return (
    <Card label="Klipper is shut down" className="border-red-500">
      <p className="mb-3 text-sm">After a restart the gains are whatever the config says.</p>
      <Button tone="danger" disabled={busy} onClick={restart}>
        Firmware restart
      </Button>
      {message !== null && (
        <p role="status" className="mt-2 text-sm text-muted-foreground">
          {message}
        </p>
      )}
    </Card>
  );
}
