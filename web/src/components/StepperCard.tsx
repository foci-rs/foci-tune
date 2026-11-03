import type { ChannelView, StepperView } from "@/api";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { titleCase } from "@/mcu";
import { cornerHz, formatAmps, formatHz, pGain, quantumUm, voltageLimitPercent } from "@/units";
import { Card, Field } from "./ui";

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section aria-label={title} className="space-y-2">
      <h3 className="text-[0.65rem] uppercase tracking-wider text-muted-foreground">{title}</h3>
      {children}
    </section>
  );
}

function percent(permille: number): string {
  return `${(permille / 10).toFixed(1)} %`;
}

function GainRow({ label, p, i, pSub, iSub }: { label: string; p: number; i: number; pSub?: string; iSub?: string }) {
  return (
    <tr className="align-top">
      <th scope="row" className="py-1 pr-4 text-left font-normal text-muted-foreground">
        {label}
      </th>
      <td className="py-1 pr-4">
        {p}
        {pSub !== undefined && <span className="block text-xs text-muted-foreground">{pSub}</span>}
      </td>
      <td className="py-1">
        {i}
        {iSub !== undefined && <span className="block text-xs text-muted-foreground">{iSub}</span>}
      </td>
    </tr>
  );
}

function Controls({ stepper, channel }: { stepper: StepperView; channel: ChannelView }) {
  const c = channel.control;
  const corner = channel.velocity_i_corner_hz ?? cornerHz(c.velocity_i, c.pwm_hz);
  return (
    <>
      <table aria-label="Gains" className="w-full font-mono text-sm">
        <thead className="border-b text-[0.65rem] uppercase tracking-wider text-muted-foreground">
          <tr>
            <th className="py-1 pr-4 text-left font-normal">Gains</th>
            <th className="py-1 pr-4 text-left font-normal">P</th>
            <th className="py-1 text-left font-normal">I</th>
          </tr>
        </thead>
        <tbody>
          <GainRow label="Torque" p={c.torque_p} i={c.torque_i} />
          <GainRow label="Flux" p={c.flux_p} i={c.flux_i} />
          <GainRow
            label="Velocity"
            p={c.velocity_p}
            pSub={pGain(c.velocity_p)}
            i={c.velocity_i}
            iSub={corner === null ? undefined : formatHz(corner, 2)}
          />
          <GainRow label="Position" p={c.position_p} pSub={pGain(c.position_p)} i={c.position_i} />
        </tbody>
      </table>
      <Separator />
      <Section title="Filters">
        <dl className="grid grid-cols-4 gap-3">
          <Field label="Torque">{c.torque_filter_hz} Hz</Field>
          <Field label="Flux">{c.flux_filter_hz} Hz</Field>
          <Field label="Velocity">{c.velocity_filter_hz} Hz</Field>
          <Field label="Position">{c.position_filter_hz} Hz</Field>
        </dl>
      </Section>
      <Separator />
      <Section title="Feedforward">
        <dl className="grid grid-cols-3 gap-3">
          <Field label="Velocity">{c.velocity_ff_enable ? percent(c.velocity_ff_permille) : "off"}</Field>
          <Field label="Acceleration">{c.accel_ff_enable ? percent(c.accel_ff_gain) : "off"}</Field>
          <Field label="Deceleration">{c.accel_ff_enable ? percent(c.decel_ff_gain) : "off"}</Field>
        </dl>
      </Section>
      <Separator />
      <Section title="Drive">
        <dl className="grid grid-cols-3 gap-3">
          <Field label="Run current">{formatAmps(c.run_current)}</Field>
          <Field label="Homing current">
            {stepper.homing_current_a === null ? "Not set" : formatAmps(stepper.homing_current_a * 1000)}
          </Field>
          <Field label="Voltage limit">{voltageLimitPercent(c.voltage_limit)}</Field>
        </dl>
      </Section>
    </>
  );
}

function UsableBadge({ stepper }: { stepper: StepperView }) {
  const usable = stepper.unusable === null;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Badge
          variant="outline"
          tabIndex={0}
          className={usable ? "border-emerald-500 text-emerald-400" : "border-amber-500 text-amber-400"}
        >
          {usable ? "Usable" : "Unusable"}
        </Badge>
      </TooltipTrigger>
      <TooltipContent side="bottom" align="end">
        {stepper.unusable ?? "The config is complete and the MCU's trace interface is open."}
      </TooltipContent>
    </Tooltip>
  );
}

export function StepperCard({ stepper, channel }: { stepper: StepperView; channel: ChannelView | undefined }) {
  return (
    <Card label={stepper.name} action={<UsableBadge stepper={stepper} />}>
      <div className="space-y-3">
        <div className="space-y-1 text-sm">
          <p className="text-muted-foreground">
            {titleCase(stepper.mcu)} · Channel {stepper.channel} · {stepper.axis.toUpperCase()} axis ·{" "}
            {stepper.position_min} to {stepper.position_max} mm · {stepper.motor_rotation_distance_mm} mm per rotation
          </p>
          {channel !== undefined && (
            <p className="text-muted-foreground">
              Encoder {channel.control.encoder_counts_per_rev} counts/rev
              {channel.quantum_units !== null &&
                ` · ${quantumUm(channel.quantum_units, stepper.motor_rotation_distance_mm).toFixed(0)} μm per count`}
            </p>
          )}
          {!stepper.electrical_model_present && <p className="text-amber-400">Electrical model missing</p>}
          {stepper.proposal.pattern === null && stepper.proposal.refusal !== null && (
            <p className="text-amber-400">{stepper.proposal.refusal}</p>
          )}
        </div>
        <Separator />
        {channel === undefined ? (
          <p className="text-sm text-muted-foreground">No control state yet.</p>
        ) : (
          <Controls stepper={stepper} channel={channel} />
        )}
      </div>
    </Card>
  );
}
