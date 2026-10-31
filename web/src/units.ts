const POSITION_UNITS_PER_REV = 65536;
const P_GAIN_DIVISOR = 256;

export function cornerHz(velocityIRaw: number, pwmHz: number): number | null {
  if (pwmHz <= 0) return null;
  return (velocityIRaw * pwmHz) / (2 * Math.PI * POSITION_UNITS_PER_REV);
}

export function velocityIRaw(cornerHz: number, pwmHz: number): number {
  return Math.round((cornerHz * 2 * Math.PI * POSITION_UNITS_PER_REV) / pwmHz);
}

export function pGain(raw: number): string {
  return (raw / P_GAIN_DIVISOR).toFixed(3);
}

export function formatUm(um: number): string {
  const rounded = Math.round(um);
  return `${rounded === 0 ? 0 : rounded} um`;
}

export function quantumUm(quantumUnits: number, rotationDistanceMm: number): number {
  return (quantumUnits / POSITION_UNITS_PER_REV) * rotationDistanceMm * 1000;
}

export function formatHz(hz: number | null, digits = 1): string {
  return hz === null ? "unknown" : `${hz.toFixed(digits)} Hz`;
}

export function formatBytes(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const text = Number.isInteger(value) ? value.toString() : value.toFixed(1);
  return `${text} ${units[unit]}`;
}

export function diffGains<T extends Record<string, number>>(
  prev: T | null,
  next: T,
): (keyof T)[] {
  if (prev === null) return [];
  return (Object.keys(next) as (keyof T)[]).filter((k) => prev[k] !== next[k]);
}
