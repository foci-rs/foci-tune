import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { DEFAULT_PREPARE, prepareState, type PrepareForm } from "@/guided";
import { TooltipProvider } from "@/components/ui/tooltip";
import { NextStepCard } from "./NextStepCard";

const control = { velocity_p: 1000, velocity_i: 50, position_p: 800, pwm_hz: 50000 };

function card(form: PrepareForm = DEFAULT_PREPARE, over: Partial<Parameters<typeof NextStepCard>[0]> = {}) {
  return (
    <TooltipProvider>
      <NextStepCard
        axis="VelocityP"
        busy={false}
        form={form}
        state={prepareState("VelocityP", form, control)}
        canReload
        onForm={() => {}}
        {...over}
      />
    </TooltipProvider>
  );
}

test("the seed starts at the live value and typing replaces it", () => {
  const onForm = vi.fn();
  render(card(DEFAULT_PREPARE, { onForm }));
  const seed = screen.getByRole("spinbutton", { name: "Seed" });
  expect(seed).toHaveValue(1000);
  fireEvent.change(seed, { target: { value: "1200" } });
  expect(onForm).toHaveBeenCalledWith({ ...DEFAULT_PREPARE, seed: 1200 });
});

test("reload goes back to the live value, and is only on offer once a seed was typed", () => {
  const onForm = vi.fn();
  const { rerender } = render(card(DEFAULT_PREPARE, { onForm }));
  expect(screen.getByRole("button", { name: "Use the live value" })).toBeDisabled();
  const typed = { ...DEFAULT_PREPARE, seed: 1200 };
  rerender(card(typed, { onForm }));
  fireEvent.click(screen.getByRole("button", { name: "Use the live value" }));
  expect(onForm).toHaveBeenCalledWith({ ...typed, seed: null });
});

test("without a live value there is nothing to reload", () => {
  render(card({ ...DEFAULT_PREPARE, seed: 900 }, { canReload: false }));
  expect(screen.getByRole("button", { name: "Use the live value" })).toBeDisabled();
});

test("the lowest and highest step are dropdowns of whole steps", () => {
  const onForm = vi.fn();
  render(card(DEFAULT_PREPARE, { onForm }));
  expect(screen.getByRole("combobox", { name: "Lowest step" })).toHaveTextContent("-3");
  expect(screen.getByRole("combobox", { name: "Highest step" })).toHaveTextContent("3");
  fireEvent.click(screen.getByRole("combobox", { name: "Highest step" }));
  fireEvent.click(screen.getByRole("option", { name: "5" }));
  expect(onForm).toHaveBeenCalledWith({ ...DEFAULT_PREPARE, upper: 5 });
});

test("the ladder shows every gain with its step, and how many runs of each", () => {
  render(card());
  const ladder = screen.getByRole("list", { name: "Ladder" });
  expect(
    within(ladder)
      .getAllByRole("listitem")
      .map((li) => li.textContent),
  ).toEqual(["-3500", "-2630", "-1794", "01000", "11260", "21587", "32000"]);
  expect(screen.getByText("7 gains, 3 repeats each")).toBeInTheDocument();
});

test("a corner ladder is in Hz", () => {
  const form = { ...DEFAULT_PREPARE, seed: 50, lower: 0, upper: 0 };
  render(card(form, { axis: "VelocityICornerHz", state: prepareState("VelocityICornerHz", form, control) }));
  expect(within(screen.getByRole("list", { name: "Ladder" })).getByText("50.0 Hz")).toBeInTheDocument();
  expect(screen.getByText("Hz", { selector: "[data-slot=input-group-addon] *" })).toBeInTheDocument();
});

test("a gain that is left out stays in the list, muted, and says why", async () => {
  const form = { ...DEFAULT_PREPARE, seed: 30000, lower: 0, upper: 1 };
  render(card(form));
  const items = within(screen.getByRole("list", { name: "Ladder" })).getAllByRole("listitem");
  expect(items.map((li) => li.textContent)).toEqual(["030000", "1left out"]);
  fireEvent.focus(within(items[1]).getByText("left out"));
  expect(await screen.findByRole("tooltip")).toHaveTextContent("outside what the stepper accepts");
});

test("a problem with the seed or range is shown instead of a ladder", () => {
  const form = { ...DEFAULT_PREPARE, seed: 0 };
  render(card(form));
  expect(screen.queryByRole("list", { name: "Ladder" })).toBeNull();
});
