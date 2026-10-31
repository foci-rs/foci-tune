//! Trace mode discovery for boards that are open but silent.
//!
//! A board in the dormant trace mode sends its boot frames once and then nothing until its
//! mode changes, so a service that starts after the board booted learns nothing from the
//! stream. Setting the mode makes the board send its metadata and one control-state frame
//! per channel.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::engine::{CommandError, Engine};
use crate::live::LiveState;
use crate::moonraker::MoonrakerApi;

/// Timing of trace mode discovery.
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    /// How long an open board may stay silent before its trace mode is set.
    pub grace: Duration,
    /// Wait after the first failed or refused attempt; doubled after each further one.
    pub retry_start: Duration,
    /// Longest wait between attempts.
    pub retry_max: Duration,
    /// How often the live state is checked.
    pub poll: Duration,
}

impl Default for DiscoveryConfig {
    /// 2 s grace, retries after 5 s doubling up to 60 s, checked every 100 ms.
    fn default() -> Self {
        Self {
            grace: Duration::from_secs(2),
            retry_start: Duration::from_secs(5),
            retry_max: Duration::from_secs(60),
            poll: Duration::from_millis(100),
        }
    }
}

/// Attempts for one board during one open of its interface.
struct Attempts {
    opens: u64,
    failures: u32,
    next_at: Instant,
    done: bool,
    unmapped_warned: bool,
}

impl Attempts {
    fn new(opens: u64, now: Instant) -> Self {
        Self {
            opens,
            failures: 0,
            next_at: now,
            done: false,
            unmapped_warned: false,
        }
    }

    fn fail(&mut self, config: &DiscoveryConfig, now: Instant) {
        let wait = config
            .retry_start
            .saturating_mul(1 << self.failures.min(16))
            .min(config.retry_max);
        self.failures += 1;
        self.next_at = now + wait;
    }
}

/// Boards open for longer than the grace period with no frame since they opened, with the
/// open count each was seen at.
fn silent_boards(live: &Mutex<LiveState>, grace: Duration, now: Instant) -> Vec<(String, u64)> {
    let live = live.lock().unwrap_or_else(PoisonError::into_inner);
    let mut silent: Vec<(String, u64)> = live
        .boards
        .iter()
        .filter(|(_, b)| {
            b.open && b.frames_since_open == 0 && now.duration_since(b.opened_at) >= grace
        })
        .map(|(serial, b)| (serial.clone(), b.opens))
        .collect();
    silent.sort();
    silent
}

fn set_note(live: &Mutex<LiveState>, serial: &str, opens: u64, note: Option<String>) {
    live.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .set_wake_note(serial, opens, note);
}

/// Sets every open board that has sent no frame within the grace period to the dormant
/// trace mode, so it reports its metadata and control state.
///
/// One command per board and interface open; a refused or failed one is retried with
/// backoff until a frame arrives or the interface closes. A board no stepper maps to is left
/// alone with a note in its unusable reason.
pub async fn run_trace_discovery<M: MoonrakerApi + 'static>(
    engine: Arc<Engine<M>>,
    live: Arc<Mutex<LiveState>>,
    config: DiscoveryConfig,
) {
    let mut boards: HashMap<String, Attempts> = HashMap::new();
    loop {
        let now = Instant::now();
        let silent = silent_boards(&live, config.grace, now);
        boards.retain(|serial, _| silent.iter().any(|(s, _)| s == serial));
        for (serial, opens) in silent {
            let attempts = boards
                .entry(serial.clone())
                .or_insert_with(|| Attempts::new(opens, now));
            if attempts.opens != opens {
                *attempts = Attempts::new(opens, now);
            }
            if attempts.done || now < attempts.next_at {
                continue;
            }
            let stepper = match engine.steppers().await {
                Ok(steppers) => steppers.into_iter().find(|s| s.serial == serial),
                Err(refused) => {
                    tracing::debug!(%serial, %refused, "trace mode discovery deferred");
                    attempts.fail(&config, Instant::now());
                    continue;
                }
            };
            let Some(stepper) = stepper else {
                if !attempts.unmapped_warned {
                    tracing::warn!(%serial, "silent trace board has no FOCI stepper");
                    attempts.unmapped_warned = true;
                }
                set_note(
                    &live,
                    &serial,
                    opens,
                    Some(format!(
                        "no FOCI stepper on board {serial}, so its trace mode cannot be set"
                    )),
                );
                attempts.fail(&config, Instant::now());
                continue;
            };
            set_note(&live, &serial, opens, None);
            match engine.wake_trace(&stepper.name).await {
                Ok(()) => {
                    tracing::info!(%serial, stepper = %stepper.name, "set a silent board dormant");
                    attempts.done = true;
                }
                Err(CommandError::Precondition(refused)) => {
                    tracing::debug!(%serial, %refused, "trace mode discovery deferred");
                    attempts.fail(&config, Instant::now());
                }
                Err(CommandError::Gcode(e)) => {
                    tracing::warn!(%serial, error = %e, "cannot set a silent board's trace mode");
                    attempts.fail(&config, Instant::now());
                }
            }
        }
        tokio::time::sleep(config.poll).await;
    }
}
