import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { RunControls } from "./GainsBlock";

test("Run is disabled and lists every failing reason", () => {
  const onRun = vi.fn();
  render(
    <RunControls
      label="Run"
      blocked={["axis x is not homed", "the trace interface of the stepper's board is not open"]}
      busy={false}
      onRun={onRun}
    />,
  );
  const button = screen.getByRole("button", { name: "Run" });
  expect(button).toBeDisabled();
  fireEvent.click(button);
  expect(onRun).not.toHaveBeenCalled();
  const reasons = within(screen.getByRole("list", { name: "Why the run cannot start" })).getAllByRole("listitem");
  expect(reasons.map((r) => r.textContent)).toEqual([
    "axis x is not homed",
    "the trace interface of the stepper's board is not open",
  ]);
});

test("Run is enabled with no reasons and runs once per press", () => {
  const onRun = vi.fn();
  render(<RunControls label="Run" blocked={[]} busy={false} onRun={onRun} />);
  fireEvent.click(screen.getByRole("button", { name: "Run" }));
  expect(onRun).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole("list")).toBeNull();
});
