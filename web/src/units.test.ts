import { describe, expect, test } from "vitest";
import {
  cornerHz,
  diffGains,
  formatBytes,
  formatUm,
  pGain,
  quantumUm,
} from "./units";

describe("units", () => {
  test("cornerHz converts the raw integrator gain at the PWM rate", () => {
    expect(cornerHz(412, 50000)).toBeCloseTo(50.03, 2);
  });

  test("cornerHz is null when the PWM rate is unknown", () => {
    expect(cornerHz(412, 0)).toBeNull();
  });

  test("pGain divides by 256 and keeps three decimals", () => {
    expect(pGain(841)).toBe("3.285");
    expect(pGain(256)).toBe("1.000");
  });

  test("formatUm rounds to whole micrometres", () => {
    expect(formatUm(123.456)).toBe("123 um");
    expect(formatUm(-0.4)).toBe("0 um");
  });

  test("quantumUm scales position units by the motor rotation distance", () => {
    expect(quantumUm(16, 40)).toBeCloseTo(9.766, 3);
  });

  test("formatBytes picks a binary unit", () => {
    expect(formatBytes(200 * 1024 * 1024)).toBe("200 MB");
    expect(formatBytes(3.5 * 1024 ** 3)).toBe("3.5 GB");
  });

  test("diffGains lists only the keys that changed", () => {
    const prev = { velocity_p: 841, velocity_i: 412, position_p: 100 };
    const next = { velocity_p: 891, velocity_i: 412, position_p: 110 };
    expect(diffGains(prev, next)).toEqual(["velocity_p", "position_p"]);
  });

  test("diffGains is empty without a previous row", () => {
    expect(diffGains(null, { velocity_p: 1 })).toEqual([]);
  });
});
