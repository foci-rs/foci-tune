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

export type ReasonlessState =
  | "queued"
  | "preparing"
  | "writing_gains"
  | "baseline"
  | "recording"
  | "moving"
  | "draining"
  | "verifying"
  | "analyzing"
  | "done"
  | "aborted"
  | "killing"
  | "kill_unconfirmed"
  | "killed";

export type ReasonState = "not_comparable" | "failed" | "skipped";

export type RunState = { state: ReasonlessState } | { state: ReasonState; reason: string };

export type RunStateName = RunState["state"];

export type CurrentRunView = { session: SessionId; run: RunId } & RunState;

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
  | { Tracking: TrackingParameters }
  | { Probe: ProbeParameters }
  | { Standstill: StandstillParameters };

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

export interface RunRegime {
  motor_rotation_distance_mm: number;
  encoder_counts_per_rev: number;
  pwm_hz: number;
  clock_freq: number;
}

export interface ChannelBaseline {
  channel: number;
  control: ControlSnapshot;
}

export interface StreamInfo {
  protocol_version: number;
  firmware_id: string;
  product: string;
  serial: string;
  clock_freq: number;
  critical_hz: number;
}

export interface StepTime {
  step: string;
  unix_ms: number;
}

export interface CaptureInfo {
  frames: number;
  file_bytes: number;
}

export type VerificationCheck = "channel_moved" | "baseline_contiguous" | "no_drop_gaps" | "edge_count";

export interface Verification {
  check: VerificationCheck;
  passed: boolean;
  detail: string;
}

export type FitOutcome =
  | {
      status: "fit";
      damped_hz: number;
      zeta: number;
      amplitude: number;
      phase: number;
      offset: number;
      tau_s: number;
      residual_rms: number;
      admissible_starts?: number;
    }
  | { status: "failed"; reason: string };

export interface TrackingRecord {
  speed_mm_s: number;
  cruise_mean_abs_um: number | null;
  sd_um: number | null;
  p99_um: number | null;
  reversal_peak_um: number | null;
  reversal_count: number;
  moving_samples: number;
}

export interface ProbeRecord {
  velocity: FitOutcome;
  torque: FitOutcome;
  position_error: FitOutcome;
  edges: number;
  stderr_at_peak: number | null;
  torque_psd_peak_hz: number | null;
}

export interface RestRecord {
  index: number;
  samples: number;
  torque_sd: number | null;
  position_pp_um: number | null;
  transitions_per_s: number | null;
  large_transitions_per_s: number | null;
}

export interface StandstillRecord {
  rests: RestRecord[];
  worst_torque_sd: number | null;
  worst_large_transitions_per_s: number | null;
}

export interface ChannelMetrics {
  channel: number;
  declared: boolean;
  tracking: TrackingRecord[];
  probe: ProbeRecord | null;
  standstill: StandstillRecord | null;
}

export interface Metrics {
  channels: ChannelMetrics[];
}

export interface RunRecord {
  format_version: number;
  id: RunId;
  request: RunRequest;
  created_unix_s: number;
  declared_channel: number;
  regime: RunRegime | null;
  baselines: ChannelBaseline[];
  stream: StreamInfo | null;
  steps: StepTime[];
  clamped_speeds: [number, number][];
  applied_speeds: number[];
  capture: CaptureInfo | null;
  verification: Verification[];
  metrics: Metrics | null;
  notes: string[];
}

export type Run = RunRecord & RunState;

export type RunEntry = { id: RunId; run: Run | null } & RunState;

export interface EnsemblePlot {
  edges: number;
  mean: (number | null)[];
  fit: (number | null)[] | null;
}

export interface ProbePlot {
  t_s: number[];
  velocity: EnsemblePlot;
  torque: EnsemblePlot;
  position_error: EnsemblePlot;
}

export interface PsdPlot {
  freq_hz: number[];
  power: (number | null)[];
}

export interface ChannelPlot {
  channel: number;
  t_s: (number | null)[];
  position_error_um: (number | null)[];
  velocity_mm_s: (number | null)[];
  torque: (number | null)[];
  ff_offset: (number | null)[];
  probe: ProbePlot | null;
  torque_psd: PsdPlot | null;
}

export interface Series {
  format_version: number;
  decimation: number;
  channels: ChannelPlot[];
}

export type LadderAxis = "velocity_p" | "velocity_i_corner_hz" | "position_p";

export interface TrackingParameters {
  speeds_mm_s: number[];
  strokes: number;
}

export interface ProbeParameters {
  speed_mm_s: number;
  high_gain: number;
  segments_between_toggles: number;
  strokes: number;
}

export interface StandstillParameters {
  rests: number;
  dwell_s: number;
}

export type RunSetup =
  | { type: "tracking"; parameters: TrackingParameters }
  | { type: "probe"; parameters: ProbeParameters }
  | { type: "standstill"; parameters: StandstillParameters };

export type RunRequestBody = {
  session: SessionId | null;
  name?: string;
  stepper: string;
  pattern: Pattern;
  gains: { velocity_p: number; velocity_i: number; position_p: number };
  filters: Filters;
} & RunSetup;

type DistributiveOmit<T, K extends PropertyKey> = T extends unknown ? Omit<T, K> : never;

export type LadderRequestBody = DistributiveOmit<RunRequestBody, "session"> & {
  axis: LadderAxis;
  values: number[];
  repeats: number;
  floor: number;
  marginal: number;
};

export interface SessionView {
  id: SessionId;
  session: Session | null;
  error: string | null;
  runs: RunEntry[];
}

export type TuneId = string;
export type TuneAxis = "VelocityP" | "VelocityICornerHz" | "PositionP";

export interface TuneGains {
  velocity_p: number;
  velocity_i: number;
  position_p: number;
  position_i: number;
}

export interface TuneGates {
  damping_floor: number;
  marginal: number;
  standstill_factor: number | null;
}

export interface TuneSetup {
  stepper: string;
  speeds_mm_s: number[];
  pattern: Pattern;
  probe: ProbeParameters;
  tracking_strokes: number;
  standstill: StandstillParameters;
  gates: TuneGates;
  include_marginal: boolean;
}

export type RungOutcome = { Assessed: number } | "Unassessable";

export interface RepeatResult {
  zeta: RungOutcome;
  p99_by_speed: (number | null)[];
  worst_torque_sd: number | null;
}

export interface RunTime {
  id: RunId;
  started_unix_ms: number;
  ended_unix_ms: number;
}

export interface RungRecord {
  shown: number;
  raw: number;
  run_ids: RunId[];
  started_unix_ms: number | null;
  ended_unix_ms: number | null;
  skipped: string | null;
  repeat_values: RepeatResult[];
  run_times: RunTime[];
  waived: { unix_ms: number; reason: Pause } | null;
  superseded: RungAttempt[];
}

export interface RungAttempt {
  run_ids: RunId[];
  run_times: RunTime[];
  repeat_values: RepeatResult[];
  ended_unix_ms: number | null;
}

export type Pause = "DampingUnassessable" | "StandstillBaselineUnassessable";

export interface PauseRecord {
  rung: number;
  reason: Pause;
  unix_ms: number;
}

export type TuneDecision = "continue" | "rerun" | "stop";

export type RungNote =
  | { OutOfRange: number }
  | { RoundedToZero: number }
  | { Merged: { shown: number; into: number } };

export type UnscoredReason = "TrackingMissing" | "StandstillMissing" | "DampingWaived";
export type InadmissibleReason = "DampingBelowFloor" | "StandstillHigh";

export interface RungVerdict {
  inadmissible: InadmissibleReason | null;
  marginal: boolean;
  unscored: UnscoredReason | null;
  score: number | null;
  deciding_speed: number | null;
  gap: number | null;
  tolerance: number | null;
}

export interface RungMedians {
  zeta: number | null;
  p99_by_speed: (number | null)[];
  worst_torque_sd: number | null;
}

export type Halt =
  | { BelowFloor: { median: number } }
  | "DampingUnassessable"
  | "StandstillBaselineUnassessable";

export interface LadderAssessment {
  verdicts: RungVerdict[];
  medians: RungMedians[];
  halt: [number, Halt] | null;
  pause?: [number, Pause] | null;
  best: number | null;
  runner_up: number | null;
  tie: number[];
  spreads: (number | null)[];
}

export type Recommendation =
  | { None: "NoScoredRung" }
  | { Measured: { rung: number } }
  | { Interpolated: { shown: number; between: [number, number] } };

export type TuneExtension = "Below" | "Above";

export interface Recommended {
  choice: Recommendation;
  extend: TuneExtension | null;
}

/**
 * Whether the seed-rung rerun matches the first run at one speed. `Drifted`: it moved by more than
 * twice the speed's relative spread. `Unassessed`: the first value, the rerun or the spread is
 * absent. `Steady`: neither.
 */
export type DriftVerdict ="Steady" | "Drifted" | "Unassessed";

export interface TuneDrift {
  metric: string;
  first: number | null;
  rerun: number | null;
  verdict: DriftVerdict | null;
}

export interface TuneStep {
  axis: TuneAxis;
  seed_raw: number;
  repeats: number;
  lower_k: number;
  upper_k: number;
  pwm_hz: number;
  base: [TuneGains, Filters];
  sessions: { probe: SessionId; tracking: SessionId; standstill: SessionId };
  rungs: RungRecord[];
  notes: RungNote[];
  chosen: number | null;
  chosen_shown: number | null;
  starts: number;
  started_unix_ms: number | null;
  ended_unix_ms: number | null;
  failed: string | null;
  run_count?: number | null;
  assessment: LadderAssessment | null;
  recommendation: Recommended | null;
  drift_rung: RungRecord | null;
  drift: TuneDrift[];
  paused: PauseRecord | null;
  standstill_gate_off: number | null;
}

export type TuneState =
  | "Ready"
  | "Announced"
  | "Running"
  | "AwaitingChoice"
  | "Paused"
  | "Done"
  | "Abandoned"
  | "RestorePending"
  | { RestoreFailed: string };

export interface Tune {
  format_version: number;
  id: TuneId;
  setup: TuneSetup;
  snapshot: [TuneGains, Filters];
  steps: TuneStep[];
  state: TuneState;
  created_unix_s: number;
  notes: { unix_ms: number; text: string }[];
}

export type TuneDetail = Tune & { config_block: string | null; lock: string | null };

export interface TuneSummary {
  id: TuneId;
  stepper: string;
  state: TuneState;
  created: number;
  current_step: TuneAxis | null;
}

export type TuneSeed = "live" | { typed: number };

export interface TuneStepRequest {
  seed: TuneSeed;
  repeats: number;
  lower_k: number;
  upper_k: number;
}

export interface TuneStepPlan {
  axis: TuneAxis;
  rungs: { values: { shown: number; raw: number }[]; notes: RungNote[] };
  run_count: number;
  announcement: string;
}

export type TuneChoiceBody = "recommended" | { rung: number } | { value: number };

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
  | ({ type: "run_state"; session: SessionId; run: RunId } & RunState)
  | { type: "run_result"; session: SessionId; run: Run }
  | { type: "queue"; runs: RunId[] }
  | { type: "tune"; tune: Tune }
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

const KILL_TIMEOUT_MS = 3000;

async function request<T>(
  method: "GET" | "POST",
  path: string,
  body?: unknown,
  timeoutMs?: number,
): Promise<T> {
  let response: Response;
  const controller = new AbortController();
  const timer = timeoutMs === undefined ? null : setTimeout(() => controller.abort(), timeoutMs);
  try {
    response = await fetch(path, {
      method,
      headers: body === undefined ? undefined : { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: controller.signal,
    });
  } catch (e) {
    if (timer !== null) clearTimeout(timer);
    const message = controller.signal.aborted
      ? "request timed out"
      : e instanceof Error
        ? e.message
        : "network error";
    throw new ApiRequestError(0, message);
  }
  if (timer !== null) clearTimeout(timer);
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

export const postKill = () => request<KillResponse>("POST", "/api/kill", undefined, KILL_TIMEOUT_MS);

export const postFirmwareRestart = () =>
  request<{ ok: true }>("POST", "/api/firmware-restart");

export const postHome = () => request<{ ok: true }>("POST", "/api/home");

export const getSessions = () => request<SessionView[]>("GET", "/api/sessions");

export const getSession = (id: SessionId) =>
  request<SessionView>("GET", `/api/sessions/${encodeURIComponent(id)}`);

export const postRun = (body: RunRequestBody) =>
  request<{ session: SessionId; run: RunId }>("POST", "/api/runs", body);

export const postLadder = (body: LadderRequestBody) =>
  request<{ session: SessionId; runs: RunId[] }>("POST", "/api/ladders", body);

export const postAbort = (run: RunId) =>
  request<{ run: RunId; aborted: RunId[] }>("POST", `/api/runs/${encodeURIComponent(run)}/abort`);

export const postRevert = (session: SessionId) =>
  request<{ session: SessionId; reverted: true }>(
    "POST",
    `/api/sessions/${encodeURIComponent(session)}/revert`,
  );

const tunePath = (id: TuneId, tail = "") => `/api/tunes/${encodeURIComponent(id)}${tail}`;

export const postTune = (body: TuneSetup) => request<Tune>("POST", "/api/tunes", body);

export const getTunes = () => request<TuneSummary[]>("GET", "/api/tunes");

export const getTune = (id: TuneId) => request<TuneDetail>("GET", tunePath(id));

export const postTuneStep = (id: TuneId, body: TuneStepRequest) =>
  request<TuneStepPlan>("POST", tunePath(id, "/steps"), body);

export const postTuneConfirm = (id: TuneId) =>
  request<{ tune: TuneId; confirmed: true }>("POST", tunePath(id, "/steps/confirm"));

export const postTuneExtension = (id: TuneId, direction: "below" | "above") =>
  request<{ tune: TuneId; confirmed: true }>("POST", tunePath(id, "/extension"), { direction });

export const postTuneChoice = (id: TuneId, choice: TuneChoiceBody) =>
  request<{ tune: TuneId; chosen: true }>("POST", tunePath(id, "/choice"), { choice });

export const postTuneDecision = (id: TuneId, decision: TuneDecision) =>
  request<unknown>("POST", tunePath(id, "/decision"), { decision });

export const postTuneAbandon = (id: TuneId) => request<unknown>("POST", tunePath(id, "/abandon"));

export const postTuneRestore = (id: TuneId) => request<unknown>("POST", tunePath(id, "/restore"));


export const getRun = (id: RunId) => request<Run>("GET", `/api/runs/${encodeURIComponent(id)}`);

export const getSeries = (id: RunId) =>
  request<Series>("GET", `/api/runs/${encodeURIComponent(id)}/series`);

export async function getConfigBlock(id: RunId): Promise<string> {
  let response: Response;
  try {
    response = await fetch(`/api/runs/${encodeURIComponent(id)}/config-block`);
  } catch (e) {
    throw new ApiRequestError(0, e instanceof Error ? e.message : "network error");
  }
  const text = await response.text();
  if (!response.ok) {
    let message = text || response.statusText;
    try {
      message = (JSON.parse(text) as { error?: string }).error ?? message;
    } catch {
    }
    throw new ApiRequestError(response.status, message);
  }
  return text;
}

const MESSAGE_TYPES: ReadonlySet<string> = new Set([
  "printer",
  "run_state",
  "run_result",
  "queue",
  "tune",
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
  let isOpen = false;

  const sendMessage = (message: ClientMessage) => {
    if (isOpen) socket?.send(JSON.stringify(message));
  };

  const open = () => {
    timer = null;
    const current = create(options.url);
    socket = current;
    current.onopen = () => {
      isOpen = true;
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
      if (socket === current) {
        socket = null;
        isOpen = false;
      }
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
