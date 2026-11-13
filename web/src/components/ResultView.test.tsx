import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import type { TuneDetail } from "@/api";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ResultView } from "./ResultView";

const BLOCK =
  "[foci stepper_x]\n" +
  "pid_velocity_p: 900      # 3.516 Q8.8\n" +
  "pid_velocity_i: 256      # 31.1 Hz corner at 50 kHz PWM\n" +
  "pid_position_p: 141      # 0.551 Q8.8\n" +
  "pid_position_i: 0\n" +
  "velocity_filter_hz: 500\n";

const step = (axis: string, chosen: number | null) => ({
  axis,
  chosen,
  chosen_shown: chosen,
  pwm_hz: 50000,
  rungs: [],
});

const tune = {
  state: "Abandoned",
  snapshot: [{ velocity_p: 841, velocity_i: 256, position_p: 141, position_i: 0 }, {}],
  steps: [step("VelocityP", 900), step("VelocityICornerHz", 256), step("PositionP", null)],
  config_block: BLOCK,
  lock: null,
} as unknown as TuneDetail;

const view = () =>
  render(
    <TooltipProvider>
      <ResultView tune={tune} block={BLOCK} live={null} />
    </TooltipProvider>,
  );

const line = (key: string) => screen.getByText(new RegExp(`^${key}:`));

test("only the line of a gain the tool changed is green", () => {
  view();
  expect(line("pid_velocity_p")).toHaveClass("text-emerald-400");
  expect(line("pid_velocity_i")).not.toHaveClass("text-emerald-400");
  expect(line("pid_position_p")).not.toHaveClass("text-emerald-400");
  expect(line("pid_position_i")).not.toHaveClass("text-emerald-400");
  expect(line("velocity_filter_hz")).not.toHaveClass("text-emerald-400");
  expect(screen.getByText(/^\[foci stepper_x\]/)).not.toHaveClass("text-emerald-400");
});

test("colouring does not change the block text", () => {
  const { container } = view();
  expect(container.querySelector("pre")?.textContent).toBe(BLOCK);
});
