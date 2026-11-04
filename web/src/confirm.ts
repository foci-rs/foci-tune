import { useCallback, useEffect, useRef, useState } from "react";

export const CONFIRM_WINDOW_MS = 2000;

export interface ConfirmState {
  armedAt: number | null;
}

export function press(
  state: ConfirmState,
  now: number,
  windowMs: number = CONFIRM_WINDOW_MS,
): { state: ConfirmState; fire: boolean } {
  if (state.armedAt !== null && now - state.armedAt <= windowMs) {
    return { state: { armedAt: null }, fire: true };
  }
  return { state: { armedAt: now }, fire: false };
}

export function expire(state: ConfirmState, now: number, windowMs: number = CONFIRM_WINDOW_MS): ConfirmState {
  if (state.armedAt !== null && now - state.armedAt > windowMs) return { armedAt: null };
  return state;
}

export function useConfirmClick(
  onConfirm: () => void,
  windowMs: number = CONFIRM_WINDOW_MS,
): { armed: boolean; click: () => void } {
  const [armed, setArmed] = useState(false);
  const state = useRef<ConfirmState>({ armedAt: null });
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const clearTimer = useCallback(() => {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
  }, []);

  useEffect(() => clearTimer, [clearTimer]);

  const click = useCallback(() => {
    const result = press(state.current, Date.now(), windowMs);
    state.current = result.state;
    clearTimer();
    if (result.fire) {
      setArmed(false);
      onConfirm();
      return;
    }
    setArmed(true);
    timer.current = setTimeout(() => {
      state.current = { armedAt: null };
      setArmed(false);
    }, windowMs + 1);
  }, [clearTimer, onConfirm, windowMs]);

  return { armed, click };
}
