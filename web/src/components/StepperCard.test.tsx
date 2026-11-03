import { fireEvent, render as baseRender, screen, within } from "@testing-library/react";
import { expect, test } from "vitest";
import type { ChannelView, StepperView } from "@/api";
import { TooltipProvider } from "@/components/ui/tooltip";
import { StepperCard } from "./StepperCard";

const render = (ui: React.ReactElement) => baseRender(<TooltipProvider>{ui}</TooltipProvider>);

const control = {
  change_sequence: 7,
  applied_tick: 1,
  applied_request: 9,
  encoder_counts_per_rev: 4000,
  pwm_hz: 50_000,
  velocity_filter_hz: 200,
  torque_filter_hz: 1000,
  position_filter_hz: 300,
  flux_filter_hz: 1000,
  velocity_p: 841,
  velocity_i: 412,
  position_p: 120,
  position_i: 4,
  torque_p: 300,
  torque_i: 20,
  flux_p: 310,
  flux_i: 21,
  velocity_ff_enable: true,
  velocity_ff_permille: 500,
  accel_ff_enable: true,
  accel_ff_gain: 120,
  decel_ff_gain: 100,
  run_current: 2500,
  voltage_limit: 0x5a81,
} as ChannelView["control"];

const channel = (over: Partial<ChannelView> = {}): ChannelView => ({
  channel: 0,
  control,
  velocity_i_corner_hz: 3.2,
  quantum_units: 16.384,
  ...over,
});

const stepper = (over: Partial<StepperView> = {}): StepperView =>
  ({
    name: "stepper_x",
    mcu: "ouroboros",
    channel: 0,
    serial: "AAA",
    axis: "x",
    position_min: 0,
    position_max: 300,
    motor_rotation_distance_mm: 40,
    electrical_model_present: true,
    unusable: null,
    homing_current_a: 0.4,
    proposal: { pattern: { start: [10, 10], end: [200, 10] }, refusal: null },
    ...over,
  }) as StepperView;

test("the card shows gains, filters, feedforward and drive values in friendly units", () => {
  const s = stepper();
  render(<StepperCard stepper={s} channel={channel()} />);
  const card = screen.getByRole("region", { name: "stepper_x" });
  expect(card).toHaveTextContent("Ouroboros · Channel 0 · X axis · 0 to 300 mm · 40 mm per rotation");
  expect(card).toHaveTextContent("Usable");
  const gains = within(card).getByRole("table", { name: "Gains" });
  expect(within(gains).getByRole("row", { name: /Velocity/ })).toHaveTextContent("841");
  expect(within(gains).getByRole("row", { name: /Velocity/ })).toHaveTextContent("412");
  expect(card).toHaveTextContent("3.20 Hz");
  expect(card).toHaveTextContent("Velocity200 Hz");
  expect(card).toHaveTextContent("50.0 %");
  expect(card).toHaveTextContent("2.50 A");
  expect(card).toHaveTextContent("Homing current0.40 A");
  expect(card).toHaveTextContent("70.7 %");
  expect(card).toHaveTextContent("Encoder 4000 counts/rev · 10 μm per count");
  expect(within(card).getByRole("region", { name: "Drive" })).not.toHaveTextContent("counts");
});

test("gains run from the inner loops outwards", () => {
  const s = stepper();
  render(<StepperCard stepper={s} channel={channel()} />);
  const rows = within(screen.getByRole("table", { name: "Gains" })).getAllByRole("row");
  expect(rows.slice(1).map((r) => r.querySelector("th")?.textContent)).toEqual([
    "Torque",
    "Flux",
    "Velocity",
    "Position",
  ]);
});

test("filters follow the same inner-to-outer order as the gains", () => {
  const s = stepper();
  render(<StepperCard stepper={s} channel={channel()} />);
  const filters = screen.getByRole("region", { name: "Filters" });
  expect(Array.from(filters.querySelectorAll("dt")).map((dt) => dt.textContent)).toEqual([
    "Torque",
    "Flux",
    "Velocity",
    "Position",
  ]);
});

test("firmware change tracking and internal units are not shown", () => {
  const s = stepper();
  render(<StepperCard stepper={s} channel={channel()} />);
  const card = screen.getByRole("region", { name: "stepper_x" });
  expect(card).not.toHaveTextContent("Last applied");
  expect(card).not.toHaveTextContent("Change sequence");
  expect(card).not.toHaveTextContent("16.384");
  expect(card).not.toHaveTextContent("PWM");
});

test("a stepper without a homing current says it is not set", () => {
  const s = stepper({ homing_current_a: null });
  render(<StepperCard stepper={s} channel={channel()} />);
  expect(screen.getByRole("region", { name: "Drive" })).toHaveTextContent("Homing currentNot set");
});

test("an unusable stepper explains itself in a tooltip, and a missing channel says so", async () => {
  const s = stepper({ unusable: "trace interface AAA not open" });
  render(<StepperCard stepper={s} channel={undefined} />);
  const card = screen.getByRole("region", { name: "stepper_x" });
  expect(card).toHaveTextContent("Unusable");
  expect(card).toHaveTextContent("No control state yet");
  fireEvent.focus(within(card).getByText("Unusable").closest("[data-slot=tooltip-trigger]")!);
  expect(await screen.findByRole("tooltip")).toHaveTextContent("trace interface AAA not open");
});

test("the usable badge says why in a tooltip", async () => {
  const s = stepper();
  render(<StepperCard stepper={s} channel={channel()} />);
  fireEvent.focus(screen.getByText("Usable").closest("[data-slot=tooltip-trigger]")!);
  expect(await screen.findByRole("tooltip")).toHaveTextContent("trace interface is open");
});

test("a missing electrical model and a refused pattern are called out", () => {
  const s = stepper({
    electrical_model_present: false,
    proposal: { pattern: null, refusal: "bed limits need stepper_y" },
  });
  render(<StepperCard stepper={s} channel={channel()} />);
  const card = screen.getByRole("region", { name: "stepper_x" });
  expect(card).toHaveTextContent("Electrical model missing");
  expect(card).toHaveTextContent("bed limits need stepper_y");
});
