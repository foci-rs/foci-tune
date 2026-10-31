export type RunId = string;
export type SessionId = string;

export interface ConfigGains {
  position_p: number | null;
  position_i: number | null;
  velocity_p: number | null;
  velocity_i: number | null;
  velocity_filter_hz: number | null;
  torque_filter_hz: number | null;
  position_filter_hz: number | null;
  flux_filter_hz: number | null;
}

export interface Pattern {
  start: [number, number];
  end: [number, number];
}

export interface ProposalView {
  pattern: Pattern | null;
  refusal: string | null;
}

export interface StepperView {
  name: string;
  mcu: string;
  channel: number;
  serial: string;
  kinematics: string;
  motor_rotation_distance_mm: number;
  axis: string;
  position_min: number;
  position_max: number;
  encoder_ppr: number;
  electrical_model_present: boolean;
  config_gains: ConfigGains;
  unusable: string | null;
  proposal: ProposalView;
}

export interface ControlSnapshot {
  change_sequence: number;
  applied_tick: number;
  applied_request: number;
  encoder_counts_per_rev: number;
  pwm_hz: number;
  velocity_meter: number;
  velocity_filter_hz: number;
  torque_filter_hz: number;
  position_filter_hz: number;
  flux_filter_hz: number;
  velocity_p: number;
  velocity_i: number;
  position_p: number;
  position_i: number;
  torque_p: number;
  torque_i: number;
  flux_p: number;
  flux_i: number;
  velocity_ff_enable: boolean;
  velocity_ff_permille: number;
  accel_ff_enable: boolean;
  accel_ff_gain: number;
  decel_ff_gain: number;
  run_current: number;
  voltage_limit: number;
}

export interface ChannelView {
  channel: number;
  control: ControlSnapshot;
  velocity_i_corner_hz: number | null;
  quantum_units: number | null;
}

export type BoardTraceMode = "unknown" | "dormant" | "streaming";

export interface BoardView {
  serial: string;
  open: boolean;
  protocol_version: number | null;
  unusable_reason: string | null;
  trace_mode: BoardTraceMode;
  critical_hz_measured: number;
  firmware_id: string | null;
  product: string | null;
  boot_changes: number;
  undecodable_frames: number;
  channels: ChannelView[];
}

export type KlippyName = "ready" | "startup" | "shutdown" | "error" | "disconnected";

export interface MoonrakerView {
  connected: boolean;
  klippy: KlippyName;
  homed_axes: string;
  print_state: string;
  idle_state: string;
  max_velocity: number;
  max_accel: number;
}

export interface PreconditionView {
  stepper: string;
  ok: boolean;
  failing: string[];
}

export type RunState =
  | { state: "queued" | "preparing" | "writing_gains" | "baseline" | "recording" }
  | { state: "moving" | "draining" | "verifying" | "analyzing" | "done" | "aborted" }
  | { state: "killing" | "kill_unconfirmed" | "killed" }
  | { state: "not_comparable" | "failed" | "skipped"; reason: string };

export interface CurrentRunView {
  session: SessionId;
  run: RunId;
  state: RunState["state"];
  reason?: string;
}

export interface EngineView {
  current: CurrentRunView | null;
  queue: RunId[];
  unconfirmed_kill: RunId | null;
}

export interface PrinterView {
  moonraker: MoonrakerView;
  steppers: StepperView[];
  settings_error: string | null;
  boards: BoardView[];
  preconditions: PreconditionView[];
  free_bytes: number | null;
  engine: EngineView;
}

export interface Gains {
  velocity_p: number;
  velocity_i: number;
  position_p: number;
  position_i: number;
}

export interface Filters {
  velocity_hz: number;
  torque_hz: number;
  position_hz: number;
  flux_hz: number;
}

export interface LadderDef {
  axis: string;
  values: number[];
  repeats: number;
  floor: number;
  marginal: number;
}

export type RunKind =
  | { Tracking: { speeds_mm_s: number[]; strokes: number } }
  | {
      Probe: {
        speed_mm_s: number;
        high_gain: number;
        segments_between_toggles: number;
        strokes: number;
      };
    }
  | { Standstill: { rests: number; dwell_s: number } };

export interface Session {
  format_version: number;
  name: string;
  created_unix_s: number;
  stepper: string;
  kind: RunKind;
  pattern: Pattern;
  gains: Gains;
  filters: Filters;
  ladder: LadderDef | null;
}

export interface RunRequest {
  session: SessionId;
  stepper: string;
  kind: RunKind;
  pattern: Pattern;
  gains: Gains;
  filters: Filters;
  ladder_value: number | null;
}

export type Run = {
  format_version: number;
  id: RunId;
  request: RunRequest;
  created_unix_s: number;
  declared_channel: number;
  regime: unknown;
  baselines: unknown[];
  stream: unknown;
  steps: unknown[];
  clamped_speeds: [number, number][];
  applied_speeds: number[];
  capture: unknown;
  verification: unknown[];
  metrics: unknown;
  notes: string[];
} & ({ state: RunState["state"]; reason?: undefined } | { state: RunState["state"]; reason: string });

export interface RunEntry {
  id: RunId;
  state: RunState["state"];
  reason?: string;
  run: Run | null;
}

export interface SessionView {
  id: SessionId;
  session: Session | null;
  error: string | null;
  runs: RunEntry[];
}

export type TraceModeCommand = "dormant" | "critical" | "diagnostic";
export type KillState = "killed" | "kill_unconfirmed";

export interface KillResponse {
  run: RunId | null;
  state: KillState;
}

export interface LiveChannel {
  channel: number;
  ticks: number[];
  step_position: number[];
  tmc_position: number[];
  position_error: number[];
  torque: number[];
  ff_offset: number[];
}

export interface LiveBatch {
  serial: string;
  clock_freq: number;
  channels: LiveChannel[];
}

export type ServerMessage =
  | ({ type: "printer" } & PrinterView)
  | { type: "run_state"; session: SessionId; run: RunId; state: RunState["state"]; reason?: string }
  | { type: "run_result"; session: SessionId; run: Run }
  | { type: "queue"; runs: RunId[] }
  | ({ type: "live" } & LiveBatch)
  | { type: "subscribed"; stream: "live" }
  | { type: "unsubscribed"; stream: "live" }
  | { type: "resync"; reason: string };

export type ClientMessage = { subscribe: "live" } | { unsubscribe: "live" };

export class ApiRequestError extends Error {
  constructor(
    readonly status: number,
    message: string,
    readonly field: string | null = null,
  ) {
    super(message);
    this.name = "ApiRequestError";
  }
}

async function request<T>(method: "GET" | "POST", path: string, body?: unknown): Promise<T> {
  let response: Response;
  try {
    response = await fetch(path, {
      method,
      headers: body === undefined ? undefined : { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch (e) {
    throw new ApiRequestError(0, e instanceof Error ? e.message : "network error");
  }
  const text = await response.text();
  let parsed: unknown = null;
  try {
    parsed = text === "" ? null : JSON.parse(text);
  } catch {
    parsed = null;
  }
  if (!response.ok) {
    const err = parsed as { error?: string; field?: string } | null;
    throw new ApiRequestError(
      response.status,
      err?.error ?? (text || response.statusText),
      err?.field ?? null,
    );
  }
  return parsed as T;
}

export const getPrinter = () => request<PrinterView>("GET", "/api/printer");

export const postTraceMode = (board: string, mode: TraceModeCommand) =>
  request<{ board: string; mode: TraceModeCommand }>("POST", "/api/trace-mode", { board, mode });

export const postKill = () => request<KillResponse>("POST", "/api/kill");

export const postFirmwareRestart = () =>
  request<{ ok: true }>("POST", "/api/firmware-restart");

export const getSessions = () => request<SessionView[]>("GET", "/api/sessions");

export const getSession = (id: SessionId) =>
  request<SessionView>("GET", `/api/sessions/${encodeURIComponent(id)}`);

const MESSAGE_TYPES: ReadonlySet<string> = new Set([
  "printer",
  "run_state",
  "run_result",
  "queue",
  "live",
  "subscribed",
  "unsubscribed",
  "resync",
]);

export function parseServerMessage(data: unknown): ServerMessage | null {
  if (typeof data !== "string") return null;
  let value: unknown;
  try {
    value = JSON.parse(data);
  } catch {
    return null;
  }
  if (typeof value !== "object" || value === null) return null;
  const type = (value as { type?: unknown }).type;
  if (typeof type !== "string" || !MESSAGE_TYPES.has(type)) return null;
  return value as ServerMessage;
}

export interface SocketLike {
  onopen: (() => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onclose: (() => void) | null;
  send(data: string): void;
  close(): void;
}

export interface EventClientOptions {
  url: string;
  onMessage: (message: ServerMessage) => void;
  onStatus?: (connected: boolean) => void;
  onResync?: (reason: string) => void;
  createSocket?: (url: string) => SocketLike;
  baseDelayMs?: number;
  maxDelayMs?: number;
}

export interface EventClient {
  subscribe(stream: "live"): void;
  unsubscribe(stream: "live"): void;
  close(): void;
}

export function eventsUrl(location: Pick<Location, "protocol" | "host">): string {
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  return `${scheme}//${location.host}/api/events`;
}

export function connectEvents(options: EventClientOptions): EventClient {
  const base = options.baseDelayMs ?? 500;
  const max = options.maxDelayMs ?? 10_000;
  const create = options.createSocket ?? ((url: string) => new WebSocket(url) as unknown as SocketLike);
  let socket: SocketLike | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let delay = base;
  let closed = false;
  let live = false;

  const sendMessage = (message: ClientMessage) => {
    socket?.send(JSON.stringify(message));
  };

  const open = () => {
    timer = null;
    const current = create(options.url);
    socket = current;
    current.onopen = () => {
      delay = base;
      options.onStatus?.(true);
      if (live) sendMessage({ subscribe: "live" });
    };
    current.onmessage = (event) => {
      const message = parseServerMessage(event.data);
      if (message === null) return;
      if (message.type === "resync") options.onResync?.(message.reason);
      options.onMessage(message);
    };
    current.onclose = () => {
      if (socket === current) socket = null;
      options.onStatus?.(false);
      if (closed) return;
      timer = setTimeout(open, delay);
      delay = Math.min(delay * 2, max);
    };
  };

  open();

  return {
    subscribe(stream) {
      live = stream === "live";
      sendMessage({ subscribe: "live" });
    },
    unsubscribe() {
      live = false;
      sendMessage({ unsubscribe: "live" });
    },
    close() {
      closed = true;
      if (timer !== null) clearTimeout(timer);
      timer = null;
      const current = socket;
      socket = null;
      current?.close();
    },
  };
}
