import { expect, test } from "vitest";
import { resetWorkbench, workbench } from "./workbench";

test("reset forgets the stored form and session link", () => {
  workbench.stepperName = "stepper_y";
  workbench.link = null;
  resetWorkbench();
  expect(Object.keys(workbench)).toEqual([]);
});
