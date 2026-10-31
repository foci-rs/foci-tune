//! In-process Moonraker stand-in: HTTP routes plus a JSON-RPC WebSocket.

// Shared by several test crates; each uses a different subset.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::sync::{Notify, broadcast, watch};
use tokio::task::JoinHandle;

#[derive(Clone, Debug)]
enum Command {
    Push(String),
    DropSockets,
}

/// What a scripted `/printer/gcode/script` call answers.
pub struct ScriptReply {
    /// `Err` carries Moonraker's error message.
    pub result: Result<(), String>,
    /// When set, the reply waits for this notification or for an emergency stop, which
    /// answers with a shutdown error as Klipper does for an interrupted script.
    pub gate: Option<Arc<Notify>>,
}

type ScriptHook = Arc<dyn Fn(&str) -> ScriptReply + Send + Sync>;

struct Inner {
    settings: Value,
    status: Value,
    scripts: Vec<String>,
    script_error: Option<String>,
    emergency_stops: u32,
    firmware_restarts: u32,
    estop_status: Option<u16>,
    estop_delay: Duration,
    estop_shutdown: Option<Duration>,
    settings_delay: Duration,
    settings_queries: u32,
    config_root: String,
    shutdown_on: Option<(String, Duration)>,
    shut_down: bool,
}

struct Shared {
    inner: Mutex<Inner>,
    refuse_ws: AtomicBool,
    commands: broadcast::Sender<Command>,
    hook: Mutex<Option<ScriptHook>>,
    estopped: watch::Sender<u64>,
}

impl Shared {
    fn push(&self, method: &str, params: Value) {
        let webhooks = match method {
            "notify_klippy_ready" => Some("ready"),
            "notify_klippy_shutdown" => Some("shutdown"),
            "notify_klippy_disconnected" => Some("disconnected"),
            _ => None,
        };
        if let Some(state) = webhooks {
            self.inner.lock().unwrap().status["webhooks"]["state"] = json!(state);
        }
        let text = json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string();
        let _ = self.commands.send(Command::Push(text));
    }

    fn call(&self, method: &str, params: &Value) -> Result<Value, String> {
        let mut inner = self.inner.lock().unwrap();
        match method {
            "server.files.roots" => Ok(json!([
                {"name": "config", "path": inner.config_root, "permissions": "rw"},
                {"name": "gcodes", "path": "/home/pi/printer_data/gcodes", "permissions": "rw"},
            ])),
            "printer.objects.query" => {
                let status = if params.get("configfile").is_some() {
                    json!({"configfile": {"settings": inner.settings.clone()}})
                } else {
                    inner.status.clone()
                };
                Ok(json!({"eventtime": 1.0, "status": status}))
            }
            "printer.objects.subscribe" => {
                if inner.status["webhooks"]["state"] == "disconnected" {
                    return Err("Klippy Disconnected".to_owned());
                }
                Ok(json!({"eventtime": 1.0, "status": inner.status.clone()}))
            }
            "printer.gcode.script" => {
                let script = params["script"].as_str().unwrap_or_default().to_owned();
                inner.scripts.push(script);
                match &inner.script_error {
                    Some(message) => Err(message.clone()),
                    None => Ok(json!("ok")),
                }
            }
            "printer.emergency_stop" => {
                inner.emergency_stops += 1;
                Ok(json!("ok"))
            }
            "printer.firmware_restart" => {
                inner.firmware_restarts += 1;
                Ok(json!("ok"))
            }
            other => Err(format!("Method not found: {other}")),
        }
    }
}

fn http_reply(result: Result<Value, String>) -> Response {
    match result {
        Ok(value) => Json(json!({"result": value})).into_response(),
        Err(message) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"code": 400, "message": message}})),
        )
            .into_response(),
    }
}

async fn roots(State(shared): State<Arc<Shared>>) -> Response {
    http_reply(shared.call("server.files.roots", &Value::Null))
}

async fn query(
    State(shared): State<Arc<Shared>>,
    Query(params): Query<serde_json::Map<String, Value>>,
) -> Response {
    if params.contains_key("configfile") {
        let delay = {
            let mut inner = shared.inner.lock().unwrap();
            inner.settings_queries += 1;
            inner.settings_delay
        };
        tokio::time::sleep(delay).await;
    }
    http_reply(shared.call("printer.objects.query", &Value::Object(params)))
}

async fn script(State(shared): State<Arc<Shared>>, Json(body): Json<Value>) -> Response {
    let notice = {
        let mut inner = shared.inner.lock().unwrap();
        let script = body["script"].as_str().unwrap_or_default().to_owned();
        let trigger = match &inner.shutdown_on {
            Some((prefix, notice)) if script.starts_with(prefix.as_str()) => Some(*notice),
            _ => None,
        };
        if inner.shut_down || trigger.is_some() {
            inner.scripts.push(script);
            inner.shut_down = true;
            inner.shutdown_on = None;
            Some(trigger)
        } else {
            None
        }
    };
    if let Some(trigger) = notice {
        if let Some(after) = trigger {
            let shared = shared.clone();
            tokio::spawn(async move {
                tokio::time::sleep(after).await;
                shared.push("notify_klippy_shutdown", json!([]));
            });
        }
        return http_reply(Err("Unknown".to_owned()));
    }
    let hook = shared.hook.lock().unwrap().clone();
    let Some(hook) = hook else {
        return http_reply(shared.call("printer.gcode.script", &body));
    };
    let script = body["script"].as_str().unwrap_or_default().to_owned();
    shared.inner.lock().unwrap().scripts.push(script.clone());
    let reply = tokio::task::spawn_blocking(move || hook(&script))
        .await
        .unwrap();
    if let Some(gate) = reply.gate {
        let mut estopped = shared.estopped.subscribe();
        tokio::select! {
            () = gate.notified() => {}
            _ = estopped.changed() => return http_reply(Err("Klipper shutdown".to_owned())),
            () = tokio::time::sleep(Duration::from_secs(30)) => {}
        }
    }
    http_reply(reply.result.map(|()| json!("ok")))
}

async fn emergency_stop(State(shared): State<Arc<Shared>>) -> Response {
    let (status, delay, shutdown) = {
        let mut inner = shared.inner.lock().unwrap();
        inner.emergency_stops += 1;
        (inner.estop_status, inner.estop_delay, inner.estop_shutdown)
    };
    tokio::time::sleep(delay).await;
    if let Some(code) = status {
        return (
            StatusCode::from_u16(code).unwrap(),
            Json(json!({"error": {"code": code, "message": "emergency stop failed"}})),
        )
            .into_response();
    }
    shared.estopped.send_modify(|n| *n += 1);
    if let Some(after) = shutdown {
        let shared = shared.clone();
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            shared.push("notify_klippy_shutdown", json!([]));
        });
    }
    http_reply(Ok(json!("ok")))
}

async fn firmware_restart(State(shared): State<Arc<Shared>>) -> Response {
    http_reply(shared.call("printer.firmware_restart", &Value::Null))
}

async fn websocket(State(shared): State<Arc<Shared>>, ws: WebSocketUpgrade) -> Response {
    if shared.refuse_ws.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let commands = shared.commands.subscribe();
    ws.on_upgrade(move |socket| serve_socket(shared, socket, commands))
}

async fn serve_socket(
    shared: Arc<Shared>,
    mut socket: WebSocket,
    mut commands: broadcast::Receiver<Command>,
) {
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Ok(Command::Push(text)) => {
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
                Ok(Command::DropSockets) | Err(_) => return,
            },
            message = socket.recv() => {
                let Some(Ok(Message::Text(text))) = message else {
                    if matches!(message, Some(Ok(_))) {
                        continue;
                    }
                    return;
                };
                let Ok(request) = serde_json::from_str::<Value>(text.as_str()) else {
                    continue;
                };
                let method = request["method"].as_str().unwrap_or_default();
                let reply = match shared.call(method, &request["params"]) {
                    Ok(result) => json!({"jsonrpc": "2.0", "result": result, "id": request["id"]}),
                    Err(message) => json!({
                        "jsonrpc": "2.0",
                        "error": {"code": -32601, "message": message},
                        "id": request["id"],
                    }),
                };
                if socket.send(Message::Text(reply.to_string().into())).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Handle to a running fake Moonraker; the server stops when it is dropped.
pub struct FakeMoonraker {
    addr: SocketAddr,
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

impl FakeMoonraker {
    pub async fn start() -> Self {
        let (commands, _) = broadcast::channel(64);
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                settings: json!({}),
                status: json!({
                    "webhooks": {"state": "ready"},
                    "toolhead": {"homed_axes": "", "max_velocity": 300.0, "max_accel": 3000.0},
                    "idle_timeout": {"state": "Idle"},
                    "print_stats": {"state": "standby"},
                }),
                scripts: Vec::new(),
                script_error: None,
                emergency_stops: 0,
                firmware_restarts: 0,
                estop_status: None,
                estop_delay: Duration::ZERO,
                estop_shutdown: None,
                settings_delay: Duration::ZERO,
                settings_queries: 0,
                config_root: "/home/pi/printer_data/config".to_owned(),
                shutdown_on: None,
                shut_down: false,
            }),
            refuse_ws: AtomicBool::new(false),
            commands,
            hook: Mutex::new(None),
            estopped: watch::channel(0).0,
        });
        let app = Router::new()
            .route("/server/files/roots", get(roots))
            .route("/printer/objects/query", get(query))
            .route("/printer/gcode/script", post(script))
            .route("/printer/emergency_stop", post(emergency_stop))
            .route("/printer/firmware_restart", post(firmware_restart))
            .route("/websocket", get(websocket))
            .with_state(shared.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { addr, shared, task }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Path reported as the `config` file root.
    pub fn set_config_root(&self, path: &str) {
        self.shared.inner.lock().unwrap().config_root = path.to_owned();
    }

    pub fn set_settings(&self, settings: Value) {
        self.shared.inner.lock().unwrap().settings = settings;
    }

    pub fn fail_scripts_with(&self, message: &str) {
        self.shared.inner.lock().unwrap().script_error = Some(message.to_owned());
    }

    pub fn scripts(&self) -> Vec<String> {
        self.shared.inner.lock().unwrap().scripts.clone()
    }

    pub fn emergency_stops(&self) -> u32 {
        self.shared.inner.lock().unwrap().emergency_stops
    }

    pub fn firmware_restarts(&self) -> u32 {
        self.shared.inner.lock().unwrap().firmware_restarts
    }

    pub fn push(&self, method: &str, params: Value) {
        self.shared.push(method, params);
    }

    /// Answers every G-code script through `hook`, which runs on a blocking thread.
    pub fn on_script(&self, hook: impl Fn(&str) -> ScriptReply + Send + Sync + 'static) {
        *self.shared.hook.lock().unwrap() = Some(Arc::new(hook));
    }

    /// Makes the emergency stop wait `delay`, then fail with `status` or succeed, and on
    /// success report Klipper shutdown `shutdown` later when set.
    pub fn script_emergency_stop(
        &self,
        status: Option<u16>,
        delay: Duration,
        shutdown: Option<Duration>,
    ) {
        let mut inner = self.shared.inner.lock().unwrap();
        inner.estop_status = status;
        inner.estop_delay = delay;
        inner.estop_shutdown = shutdown;
    }

    /// Shuts Klipper down from another client when the first script starting with `prefix`
    /// arrives: that script and every later one fail as Moonraker answers them in shutdown,
    /// and the shutdown notice follows on the socket `notice` later.
    pub fn shut_down_on(&self, prefix: &str, notice: Duration) {
        self.shared.inner.lock().unwrap().shutdown_on = Some((prefix.to_owned(), notice));
    }

    /// How many `configfile.settings` queries have arrived, answered or not.
    pub fn settings_queries(&self) -> u32 {
        self.shared.inner.lock().unwrap().settings_queries
    }

    /// Delays every `configfile.settings` query by `delay`.
    pub fn delay_settings(&self, delay: Duration) {
        self.shared.inner.lock().unwrap().settings_delay = delay;
    }

    /// Merges `patch` into the subscribed objects without notifying anyone.
    pub fn set_status(&self, patch: Value) {
        let mut inner = self.shared.inner.lock().unwrap();
        for (object, fields) in patch.as_object().unwrap() {
            for (key, value) in fields.as_object().unwrap() {
                inner.status[object][key] = value.clone();
            }
        }
    }

    /// Merges `patch` and pushes it as a status update.
    pub fn update_status(&self, patch: Value) {
        self.set_status(patch.clone());
        self.push("notify_status_update", json!([patch, 2.0]));
    }

    pub fn refuse_websockets(&self, refuse: bool) {
        self.shared.refuse_ws.store(refuse, Ordering::SeqCst);
    }

    pub fn drop_websockets(&self) {
        let _ = self.shared.commands.send(Command::DropSockets);
    }
}

impl Drop for FakeMoonraker {
    fn drop(&mut self) {
        self.task.abort();
    }
}
