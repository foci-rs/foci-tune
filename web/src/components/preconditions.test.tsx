import { render, screen, within } from "@testing-library/react";
import { expect, test } from "vitest";
import { PreconditionList } from "./preconditions";

test("names every failing item for a stepper", () => {
  render(
    <PreconditionList
      items={[
        {
          stepper: "stepper_x",
          ok: false,
          failing: ["axis x is not homed", "the trace interface of the stepper's board is not open"],
        },
        { stepper: "stepper_y", ok: true, failing: [] },
      ]}
    />,
  );
  const x = screen.getByRole("group", { name: "stepper_x" });
  const rows = within(x).getAllByRole("listitem");
  expect(rows.map((r) => r.textContent)).toEqual([
    "axis x is not homed",
    "the trace interface of the stepper's board is not open",
  ]);
  const y = screen.getByRole("group", { name: "stepper_y" });
  expect(within(y).queryAllByRole("listitem")).toHaveLength(0);
  expect(y).toHaveTextContent("All preconditions hold");
});
