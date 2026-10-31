//! Moonraker client: HTTP for requests, a WebSocket for subscriptions and events.

use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Message, http::HeaderValue};

/// Bound on establishing a connection to Moonraker.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Bound on a whole request, except G-code scripts, which return only when motion ends.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Failure talking to Moonraker.
#[derive(Debug, thiserror::Error)]
pub enum MoonrakerError {
    /// The HTTP request could not be completed.
    #[error("moonraker request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// Moonraker answered with an error; the text is its own message.
    #[error("{0}")]
    Api(String),
    /// Moonraker's reply did not have the expected shape.
    #[error("unexpected moonraker reply: {0}")]
    Protocol(String),
}

/// Result alias for Moonraker calls.
pub type Result<T> = std::result::Result<T, MoonrakerError>;

/// Klipper's state as Moonraker reports it, plus the tool's own `Disconnected`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KlippyState {
    /// Klipper is ready.
    Ready,
    /// Klipper is starting up.
    Startup,
    /// Klipper is in shutdown.
    Shutdown,
    /// Klipper reported a startup error.
    Error,
    /// No Moonraker socket, or Moonraker lost its Klipper connection.
    Disconnected,
}

impl KlippyState {
    fn from_webhooks(state: &str) -> Self {
        match state {
            "ready" => Self::Ready,
            "shutdown" => Self::Shutdown,
            "error" => Self::Error,
            _ => Self::Startup,
        }
    }
}

/// Printer facts kept live from the subscription.
#[derive(Debug, Clone, PartialEq)]
pub struct PrinterState {
    /// Klipper state.
    pub klippy: KlippyState,
    /// Axes Klipper considers homed, for example `"xy"`.
    pub homed_axes: String,
    /// `idle_timeout.state`.
    pub idle_state: String,
    /// `print_stats.state`.
    pub print_state: String,
    /// Toolhead `max_velocity`, mm/s.
    pub max_velocity: f64,
    /// Toolhead `max_accel`, mm/s^2.
    pub max_accel: f64,
}

impl PrinterState {
    fn disconnected() -> Self {
        Self {
            klippy: KlippyState::Disconnected,
            homed_axes: String::new(),
            idle_state: String::new(),
            print_state: String::new(),
            max_velocity: 0.0,
            max_accel: 0.0,
        }
    }

    fn apply(&mut self, status: &Value) {
        if let Some(s) = status["webhooks"]["state"].as_str() {
            self.klippy = KlippyState::from_webhooks(s);
        }
        if let Some(s) = status["toolhead"]["homed_axes"].as_str() {
            self.homed_axes = s.to_owned();
        }
        if let Some(v) = status["toolhead"]["max_velocity"].as_f64() {
            self.max_velocity = v;
        }
        if let Some(v) = status["toolhead"]["max_accel"].as_f64() {
            self.max_accel = v;
        }
        if let Some(s) = status["idle_timeout"]["state"].as_str() {
            self.idle_state = s.to_owned();
        }
        if let Some(s) = status["print_stats"]["state"].as_str() {
            self.print_state = s.to_owned();
        }
    }
}

/// Connection settings for [`MoonrakerClient`].
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Base URL, for example `http://127.0.0.1:7125`.
    pub url: String,
    /// Optional API key for installs with authentication on.
    pub api_key: Option<String>,
    /// First reconnect delay; doubles after each failed attempt.
    pub backoff_initial: Duration,
    /// Ceiling for the reconnect delay.
    pub backoff_max: Duration,
}

impl ClientConfig {
    /// Defaults: 250 ms first retry, capped at 5 s.
    pub fn new(url: String, api_key: Option<String>) -> Self {
        Self {
            url,
            api_key,
            backoff_initial: Duration::from_millis(250),
            backoff_max: Duration::from_secs(5),
        }
    }
}

/// The Moonraker operations the rest of the service depends on.
pub trait MoonrakerApi: Send + Sync {
    /// Filesystem path of the `config` file root.
    fn config_root(&self) -> impl Future<Output = Result<PathBuf>> + Send;
    /// The `configfile.settings` object.
    fn settings(&self) -> impl Future<Output = Result<Value>> + Send;
    /// Run G-code; an error carries Moonraker's message.
    fn script(&self, gcode: &str) -> impl Future<Output = Result<()>> + Send;
    /// Trigger an emergency stop.
    fn emergency_stop(&self) -> impl Future<Output = Result<()>> + Send;
    /// Trigger a Klipper firmware restart.
    fn firmware_restart(&self) -> impl Future<Output = Result<()>> + Send;
    /// Live printer state.
    fn state(&self) -> watch::Receiver<PrinterState>;
    /// G-code console lines pushed by Moonraker.
    fn gcode_responses(&self) -> broadcast::Receiver<String>;
    /// Count of Klipper restarts Moonraker reported: a `notify_klippy_disconnected` followed
    /// later by `notify_klippy_ready`. The tool's own reconnects never change it.
    fn klippy_restarts(&self) -> watch::Receiver<u64>;
}

/// Moonraker client with a background task that keeps the WebSocket subscribed.
pub struct MoonrakerClient {
    http: reqwest::Client,
    base: String,
    api_key: Option<String>,
    state: watch::Receiver<PrinterState>,
    gcode: broadcast::Sender<String>,
    restarts: watch::Receiver<u64>,
    task: JoinHandle<()>,
}

impl MoonrakerClient {
    /// Start the client; the connection is established and kept up in the background.
    pub fn connect(config: ClientConfig) -> Self {
        let base = config.url.trim_end_matches('/').to_owned();
        let (state_tx, state) = watch::channel(PrinterState::disconnected());
        let (gcode, _) = broadcast::channel(256);
        let (restarts_tx, restarts) = watch::channel(0);
        let task = tokio::spawn(run_socket(
            config.clone(),
            state_tx,
            gcode.clone(),
            restarts_tx,
        ));
        Self {
            http: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .build()
                .unwrap_or_default(),
            base,
            api_key: config.api_key,
            state,
            gcode,
            restarts,
            task,
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let builder = self.http.request(method, format!("{}{path}", self.base));
        match &self.api_key {
            Some(key) => builder.header("X-Api-Key", key),
            None => builder,
        }
    }

    async fn result(request: reqwest::RequestBuilder) -> Result<Value> {
        let response = request.send().await?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if let Some(message) = body["error"]["message"].as_str() {
            return Err(MoonrakerError::Api(message.to_owned()));
        }
        if !status.is_success() {
            return Err(MoonrakerError::Api(format!("moonraker returned {status}")));
        }
        Ok(body["result"].clone())
    }

    async fn action(&self, path: &str) -> Result<()> {
        Self::result(
            self.request(reqwest::Method::POST, path)
                .timeout(REQUEST_TIMEOUT),
        )
        .await?;
        Ok(())
    }
}

impl Drop for MoonrakerClient {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MoonrakerApi for MoonrakerClient {
    async fn config_root(&self) -> Result<PathBuf> {
        let roots = Self::result(
            self.request(reqwest::Method::GET, "/server/files/roots")
                .timeout(REQUEST_TIMEOUT),
        )
        .await?;
        roots
            .as_array()
            .into_iter()
            .flatten()
            .find(|root| root["name"] == "config")
            .and_then(|root| root["path"].as_str())
            .map(PathBuf::from)
            .ok_or_else(|| MoonrakerError::Protocol("no config file root".to_owned()))
    }

    async fn settings(&self) -> Result<Value> {
        let reply = Self::result(
            self.request(reqwest::Method::GET, "/printer/objects/query")
                .query(&[("configfile", "settings")])
                .timeout(REQUEST_TIMEOUT),
        )
        .await?;
        let settings = reply["status"]["configfile"]["settings"].clone();
        if settings.is_null() {
            return Err(MoonrakerError::Protocol(
                "configfile settings missing".to_owned(),
            ));
        }
        Ok(settings)
    }

    async fn script(&self, gcode: &str) -> Result<()> {
        let reply = Self::result(
            self.request(reqwest::Method::POST, "/printer/gcode/script")
                .json(&json!({"script": gcode})),
        )
        .await?;
        match reply.as_str() {
            Some(text) if text.starts_with("!!") => Err(MoonrakerError::Api(text.to_owned())),
            _ => Ok(()),
        }
    }

    async fn emergency_stop(&self) -> Result<()> {
        self.action("/printer/emergency_stop").await
    }

    async fn firmware_restart(&self) -> Result<()> {
        self.action("/printer/firmware_restart").await
    }

    fn state(&self) -> watch::Receiver<PrinterState> {
        self.state.clone()
    }

    fn gcode_responses(&self) -> broadcast::Receiver<String> {
        self.gcode.subscribe()
    }

    fn klippy_restarts(&self) -> watch::Receiver<u64> {
        self.restarts.clone()
    }
}

/// Counts Klipper restarts from Moonraker's own notifications, across socket sessions.
struct RestartTracker {
    count: watch::Sender<u64>,
    klippy_gone: bool,
}

impl RestartTracker {
    fn disconnected(&mut self) {
        self.klippy_gone = true;
    }

    fn ready(&mut self) {
        if std::mem::take(&mut self.klippy_gone) {
            self.count.send_modify(|n| *n += 1);
        }
    }
}

fn websocket_request(
    config: &ClientConfig,
) -> Option<tokio_tungstenite::tungstenite::handshake::client::Request> {
    let base = config.url.trim_end_matches('/');
    let ws_url = match base.strip_prefix("https://") {
        Some(rest) => format!("wss://{rest}/websocket"),
        None => format!(
            "ws://{}/websocket",
            base.strip_prefix("http://").unwrap_or(base)
        ),
    };
    let mut request = ws_url.into_client_request().ok()?;
    if let Some(key) = &config.api_key {
        request
            .headers_mut()
            .insert("X-Api-Key", HeaderValue::from_str(key).ok()?);
    }
    Some(request)
}

fn subscribe_message(id: u64) -> Message {
    json!({
        "jsonrpc": "2.0",
        "method": "printer.objects.subscribe",
        "params": {"objects": {
            "toolhead": ["homed_axes", "max_velocity", "max_accel"],
            "idle_timeout": ["state"],
            "print_stats": ["state"],
            "webhooks": ["state"],
        }},
        "id": id,
    })
    .to_string()
    .into()
}

async fn run_socket(
    config: ClientConfig,
    state: watch::Sender<PrinterState>,
    gcode: broadcast::Sender<String>,
    restarts: watch::Sender<u64>,
) {
    let mut restarts = RestartTracker {
        count: restarts,
        klippy_gone: false,
    };
    let Some(request) = websocket_request(&config) else {
        tracing::error!(
            "moonraker url is not usable for a websocket: {}",
            config.url
        );
        return;
    };
    let mut delay = config.backoff_initial;
    loop {
        match tokio_tungstenite::connect_async(request.clone()).await {
            Ok((socket, _)) => {
                delay = config.backoff_initial;
                session(socket, &state, &gcode, &mut restarts).await;
            }
            Err(error) => tracing::debug!("moonraker websocket connect failed: {error}"),
        }
        state.send_replace(PrinterState::disconnected());
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(config.backoff_max);
    }
}

async fn session<S>(
    mut socket: tokio_tungstenite::WebSocketStream<S>,
    state: &watch::Sender<PrinterState>,
    gcode: &broadcast::Sender<String>,
    restarts: &mut RestartTracker,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    state.send_modify(|s| s.klippy = KlippyState::Startup);
    let mut next_id = 1;
    if socket.send(subscribe_message(next_id)).await.is_err() {
        return;
    }
    while let Some(Ok(message)) = socket.next().await {
        let Message::Text(text) = message else {
            continue;
        };
        let Ok(frame) = serde_json::from_str::<Value>(text.as_str()) else {
            continue;
        };
        let mut resubscribe = false;
        match frame["method"].as_str() {
            Some("notify_status_update") => {
                state.send_modify(|s| s.apply(&frame["params"][0]));
            }
            Some("notify_gcode_response") => {
                if let Some(line) = frame["params"][0].as_str() {
                    let _ = gcode.send(line.to_owned());
                }
            }
            Some("notify_klippy_ready") => {
                restarts.ready();
                state.send_modify(|s| s.klippy = KlippyState::Ready);
                resubscribe = true;
            }
            Some("notify_klippy_shutdown") => {
                state.send_modify(|s| s.klippy = KlippyState::Shutdown);
            }
            Some("notify_klippy_disconnected") => {
                restarts.disconnected();
                state.send_modify(|s| s.klippy = KlippyState::Disconnected);
            }
            Some(_) => {}
            None => {
                if frame["result"]["status"].is_object() {
                    state.send_modify(|s| s.apply(&frame["result"]["status"]));
                }
            }
        }
        if resubscribe {
            next_id += 1;
            if socket.send(subscribe_message(next_id)).await.is_err() {
                return;
            }
        }
    }
}
