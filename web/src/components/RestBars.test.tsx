import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import type { RestRecord } from "@/api";
import { RestBars } from "./RestBars";

const rest = (index: number, over: Partial<RestRecord>): RestRecord => ({
  index,
  samples: 100,
  torque_sd: null,
  position_pp_um: null,
  transitions_per_s: null,
  large_transitions_per_s: null,
  ...over,
});

test("the rest table lists the transition rates beside the torque sd", () => {
  const value = [
    rest(0, { torque_sd: 0.5, transitions_per_s: 950.2, large_transitions_per_s: 0 }),
    rest(1, {}),
  ];
  render(<RestBars runs={[{ id: "r1", label: "run 1", color: "#000", value }]} />);
  expect(screen.getByRole("rowheader", { name: /torque sd/ })).toBeInTheDocument();
  expect(screen.getByRole("rowheader", { name: /transitions \/s/ })).toBeInTheDocument();
  expect(screen.getByRole("rowheader", { name: /large steps \/s/ })).toBeInTheDocument();
  expect(screen.getByText("950.2")).toBeInTheDocument();
  expect(screen.getByText("0.0")).toBeInTheDocument();
  expect(screen.getAllByText("n/a").length).toBeGreaterThanOrEqual(3);
});
