// SPDX-License-Identifier: GPL-3.0-or-later
mod adapters;
mod network;
mod notifications;
mod scene;
mod wifi;
use anyhow::{Context, Result, bail, ensure};
use notifications::NotificationServer;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use xr_shell_protocols::Client;
use xr_spatial_kit::{Action, InteractionState, Kind, Phase, PointerEvent, Widget};

enum Work {
    System(String, Option<String>),
    Wifi(network::Request),
}
struct Requests(mpsc::Sender<Work>);
impl Requests {
    fn send(&self, (name, value): (String, Option<String>)) -> Result<()> {
        self.0
            .send(Work::System(name, value))
            .map_err(|_| anyhow::anyhow!("system worker stopped"))
    }
    fn wifi(&self, request: network::Request) -> Result<()> {
        self.0
            .send(Work::Wifi(request))
            .map_err(|_| anyhow::anyhow!("system worker stopped"))
    }
}

fn input_action(
    widgets: &mut BTreeMap<String, Widget>,
    interactions: &mut BTreeMap<String, InteractionState>,
    payload: &Value,
    cancel: bool,
) -> Option<(String, Option<f32>)> {
    if cancel {
        if let Some(id) = payload["target_node_id"].as_str() {
            if let Some(state) = interactions.get_mut(id) {
                state.cancel();
            }
        } else {
            for state in interactions.values_mut() {
                state.cancel();
            }
        }
        return None;
    }
    let id = payload["target_node_id"].as_str()?;
    let widget = widgets.get_mut(id)?;
    let phase = match payload["phase"].as_str()? {
        "hover" => Phase::Hover,
        "press" => Phase::Press,
        "drag" => Phase::Drag,
        "release" => Phase::Release,
        _ => return None,
    };
    let interaction = payload["interaction_id"].as_str()?;
    let event = PointerEvent {
        phase,
        interaction_id: interaction.into(),
        local: [
            payload["local_x"].as_f64()? as f32,
            payload["local_y"].as_f64()? as f32,
        ],
    };
    let state = interactions.entry(id.into()).or_default();
    let matching = state.pressed && state.interaction_id.as_deref() == Some(interaction);
    match state.apply(widget, &event) {
        Some(Action::Activated(action)) => Some((action, None)),
        _ if phase == Phase::Release
            && matching
            && widget.enabled
            && widget.kind == Kind::Slider =>
        {
            // Commit one final value after a captured gesture; intermediate drag
            // values stay in the widget instead of queuing OS writes each frame.
            Some((widget.action_id.clone(), Some(widget.value)))
        }
        _ => None,
    }
}

struct Options {
    mode: String,
    action: Option<String>,
    value: Option<String>,
    socket: Option<PathBuf>,
    seconds: u64,
    state_file: Option<PathBuf>,
    isolated_notifications: bool,
}
fn options() -> Result<Options> {
    let mut args = std::env::args().skip(1).peekable();
    let mode = if args.peek().is_some_and(|s| !s.starts_with('-')) {
        args.next().unwrap()
    } else {
        "run".into()
    };
    let mut o = Options {
        mode,
        action: None,
        value: None,
        socket: None,
        seconds: 0,
        state_file: None,
        isolated_notifications: false,
    };
    if o.mode == "action" {
        o.action = args.next();
        if args.peek().is_some_and(|s| !s.starts_with('-')) {
            o.value = args.next();
        }
    }
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => o.socket = Some(args.next().context("--socket needs path")?.into()),
            "--seconds" => o.seconds = args.next().context("--seconds needs duration")?.parse()?,
            "--state-file" => {
                o.state_file = Some(args.next().context("--state-file needs path")?.into())
            }
            "--isolated-notifications" => o.isolated_notifications = true,
            "--help" | "-h" => {
                println!(
                    "xr-system-ui probe\nxr-system-ui action theme|volume|mute|ime VALUE\nxr-system-ui action taskbar fixed|follow --socket PATH\nxr-system-ui action workspace|clipboard|text|key VALUE --socket PATH\nxr-system-ui run --socket PATH [--seconds N] [--state-file FILE] [--isolated-notifications]\nNotification ownership requires XR_SYSTEM_UI_ISOLATED_BUS=1 and a private bus."
                );
                std::process::exit(0)
            }
            _ => bail!("unknown argument {arg}"),
        }
    }
    Ok(o)
}
fn socket(o: &Options) -> Result<PathBuf> {
    o.socket
        .clone()
        .or_else(|| {
            std::env::var_os("XDG_RUNTIME_DIR")
                .map(|p| PathBuf::from(p).join("xr-desktop/shell.sock"))
        })
        .context("--socket or XDG_RUNTIME_DIR required")
}
fn connect(o: &Options) -> Result<Client> {
    Client::connect(
        &socket(o)?,
        "system_ui",
        &[
            "core.v1",
            "scene.v1",
            "scene.spatial-policy.v1",
            "scene.spherical-placement.v1",
            "taskbar.v1",
            "snapshot.v1",
            "input.v1",
            "system.v1",
            "scene.horizon-spawn.v1",
        ],
    )
}
fn write_state(path: &Option<PathBuf>, value: &Value) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(path) = path {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(serde_json::to_vec_pretty(value)?.as_slice())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(tmp, path)?;
    }
    Ok(())
}
fn preference_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("XR_SYSTEM_UI_PREFERENCES_FILE").filter(|p| !p.is_empty())
    {
        return Some(PathBuf::from(path));
    }
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
        .map(|p| p.join("xr-system-ui/preferences.json"))
}
fn read_taskbar_mode() -> String {
    preference_path()
        .and_then(|p| fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| {
            v["taskbar_mode"]
                .as_str()
                .filter(|m| ["fixed", "follow"].contains(m))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "fixed".into())
}
fn save_taskbar_mode(mode: &str) -> Result<()> {
    write_state(&preference_path(), &json!({"schema":1,"taskbar_mode":mode}))
}
fn action_error(message: &Value) -> Result<()> {
    if ["Error", "SceneRejected"].contains(&message["type"].as_str().unwrap_or("")) {
        bail!("Shell rejected request: {}", message["payload"]);
    }
    Ok(())
}
fn shell_action(o: &Options, name: &str, value: &str) -> Result<Value> {
    let mut client = connect(o)?;
    let (kind, payload, expected) = match name {
        "taskbar" => {
            ensure!(
                ["fixed", "follow"].contains(&value),
                "taskbar mode must be fixed or follow"
            );
            ("TaskbarIntent", json!({"mode":value}), "TaskbarApplied")
        }
        "workspace" => {
            let n: u64 = value.parse()?;
            ensure!(n < 4, "workspace must be 0..3");
            (
                "WorkspaceIntent",
                json!({"workspace":n}),
                "WorkspaceApplied",
            )
        }
        "clipboard" => {
            ensure!(value.len() <= 60000, "clipboard text too long");
            (
                "ClipboardSet",
                json!({"mime_type":"text/plain;charset=utf-8","text":value}),
                "ClipboardChanged",
            )
        }
        "text" => ("TextCommit", json!({"text":value}), "InputAccepted"),
        "key" => {
            let key: u32 = value.parse()?;
            ensure!(key <= 767, "invalid evdev keycode");
            client.send("KeyIntent", json!({"keycode":key,"pressed":true}), &[])?;
            (
                "KeyIntent",
                json!({"keycode":key,"pressed":false}),
                "InputAccepted",
            )
        }
        _ => bail!("unsupported Shell action"),
    };
    let request = client.send(kind, payload, &[])?;
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        if let Some(packet) = client.recv(100)? {
            action_error(&packet.message)?;
            if packet.message["type"] == expected
                && (expected == "ClipboardChanged" || packet.message["request_id"] == request)
            {
                if expected == "ClipboardChanged" {
                    ensure!(
                        packet.message["payload"]["text"] == value,
                        "clipboard readback mismatch"
                    );
                }
                if expected == "TaskbarApplied" {
                    save_taskbar_mode(value)?;
                }
                return Ok(
                    json!({"action":name,"requested":value,"observed":packet.message["payload"],
                    "client_text_verified":false,"verification":if expected=="InputAccepted"{"routed_to_acknowledged_focus"}else{"state_readback"}}),
                );
            }
        }
    }
    bail!("timed out waiting for actual {expected}")
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    let result = run().await;
    if let Err(e) = &result {
        eprintln!("XR_SYSTEM_UI_ERROR={e:#}");
    }
    result
}
async fn run() -> Result<()> {
    let o = options()?;
    if o.mode == "probe" {
        println!("{}", serde_json::to_string_pretty(&adapters::probe())?);
        return Ok(());
    }
    if o.mode == "action" {
        let name = o.action.as_deref().context("action name required")?;
        let result = if ["workspace", "clipboard", "text", "key", "taskbar"].contains(&name) {
            shell_action(
                &o,
                name,
                o.value.as_deref().context("action value required")?,
            )
        } else {
            adapters::action(name, o.value.as_deref())
        };
        match &result {
            Ok(value) => println!("{}", json!({"success":true,"result":value})),
            Err(e) => println!("{}", json!({"success":false,"error":format!("{e:#}")})),
        };
        result?;
        return Ok(());
    }
    ensure!(o.mode == "run", "expected probe, action or run");
    let server = if o.isolated_notifications {
        Some(NotificationServer::isolated().await?)
    } else {
        None
    };
    let quit = Arc::new(AtomicBool::new(false));
    let signal = quit.clone();
    ctrlc::set_handler(move || {
        signal.store(true, Ordering::Relaxed);
    })?;
    let (requests, receiver) = mpsc::channel::<Work>();
    let (updates, events) = mpsc::channel::<Value>();
    let stopped = quit.clone();
    let worker = std::thread::spawn(move || {
        let mut next_probe = Instant::now();
        while !stopped.load(Ordering::Relaxed) {
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(work) => {
                    let wifi = matches!(&work, Work::Wifi(_));
                    let result = match work {
                        Work::System(name, value) => adapters::action(&name, value.as_deref()),
                        Work::Wifi(request) => network::execute(request),
                    };
                    let update = match result {
                        Ok(value) => {
                            json!({"type":if wifi {"wifi"} else {"action"},"success":true,"result":value})
                        }
                        Err(e) => {
                            json!({"type":if wifi {"wifi"} else {"action"},"success":false,"error":format!("{e:#}")})
                        }
                    };
                    if updates.send(update).is_err() {
                        break;
                    }
                    next_probe = Instant::now();
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if Instant::now() >= next_probe {
                if updates
                    .send(json!({"type":"probe","state":adapters::probe()}))
                    .is_err()
                {
                    break;
                }
                next_probe = Instant::now() + Duration::from_secs(2);
            }
        }
    });
    let result = scene_loop(&o, &quit, server.as_ref(), Requests(requests), events).await;
    quit.store(true, Ordering::Relaxed);
    let _ = worker.join();
    if let Some(server) = server {
        server
            .connection
            .release_name("org.freedesktop.Notifications")
            .await?;
    }
    println!("XR_SYSTEM_UI_CLEANUP=1");
    result
}

async fn scene_loop(
    o: &Options,
    quit: &AtomicBool,
    server: Option<&NotificationServer>,
    requests: Requests,
    updates: mpsc::Receiver<Value>,
) -> Result<()> {
    let mut client = connect(o)?;
    let preferred_mode = read_taskbar_mode();
    let mut taskbar_request =
        Some(client.send("TaskbarIntent", json!({"mode":preferred_mode}), &[])?);
    let mut taskbar_mode = "fixed".to_string();
    client.send("GetSnapshot", json!({}), &[])?;
    client.send("GetClipboard", json!({}), &[])?;
    let start = Instant::now();
    let mut state = json!({});
    let mut active_workspace = None;
    let mut clipboard: Option<String> = None;
    let mut last_result = "正在探测实际系统接口".to_string();
    let mut revision = 0u64;
    let mut pending = false;
    let mut sent_scene = Value::Null;
    let mut scene_dirty = true;
    let mut node_ids = BTreeSet::new();
    let mut widgets = BTreeMap::<String, Widget>::new();
    let mut interactions = BTreeMap::<String, InteractionState>::new();
    let mut last_system = Value::Null;
    let mut requests_by_id = BTreeMap::<String, String>::new();
    let mut panels = scene::Panels::default();
    let mut wifi = wifi::Wifi::default();
    while !quit.load(Ordering::Relaxed)
        && (o.seconds == 0 || start.elapsed() < Duration::from_secs(o.seconds))
    {
        if let Some(server) = server {
            server.expire().await?;
        }
        let all_notices = server.map(|s| s.store.snapshot()).unwrap_or_default();
        let notification_count = all_notices.len();
        // Keep metadata below the negotiated message limit even when notices
        // contain their maximum body/action payloads. The D-Bus store retains all.
        let notices = all_notices
            .into_iter()
            .rev()
            .take(2)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>();
        while let Ok(update) = updates.try_recv() {
            if update["type"] == "probe" {
                state = update["state"].clone();
                scene_dirty = true;
            } else if update["type"] == "wifi" {
                wifi.busy = None;
                wifi.message = if update["success"] == true {
                    state["network"] =
                        json!({"available":true,"state":update["result"]["observed"]});
                    match update["result"]["action"].as_str() {
                        Some("connect") => {
                            wifi.clear_secret();
                            if let Some(selected) = &wifi.selected {
                                if let Some(fresh) =
                                    wifi::access_points(&state).into_iter().find(|ap| {
                                        ap.device == selected.device
                                            && ap.ssid_bytes == selected.ssid_bytes
                                            && ap.key_mgmt == selected.key_mgmt
                                    })
                                {
                                    wifi.selected = Some(fresh);
                                }
                            }
                            "连接成功".into()
                        }
                        Some("scan") => "扫描完成".into(),
                        Some("radio") => "无线开关已更新".into(),
                        Some("disconnect") => {
                            wifi.back();
                            "已断开连接".into()
                        }
                        _ => String::new(),
                    }
                } else {
                    wifi.revealed = false;
                    update["error"]
                        .as_str()
                        .unwrap_or("操作失败，请重试")
                        .into()
                };
                println!(
                    "XR_WIFI_RESULT={}",
                    json!({"success":update["success"],"action":update["result"]["action"]})
                );
                scene_dirty = true;
            } else {
                last_result = if update["success"] == true {
                    format!("已读回系统操作：{}", update["result"]["action"])
                } else {
                    format!("操作失败：{}", update["error"])
                };
                println!("XR_SYSTEM_ACTION_RESULT={update}");
                scene_dirty = true;
            }
        }
        if !panels.wifi && wifi.selected.is_some() {
            wifi.back();
        }
        if let Some(selected) = &wifi.selected {
            if let Some(fresh) = wifi::access_points(&state).into_iter().find(|ap| {
                ap.device == selected.device
                    && ap.ssid_bytes == selected.ssid_bytes
                    && ap.key_mgmt == selected.key_mgmt
            }) {
                wifi.selected = Some(fresh);
            }
        }
        let mut system = state.clone();
        system["notifications"] = json!({"available":server.is_some(),"owned_in_isolated_bus":server.is_some(),"count":notification_count,"entries":notices});
        system["clipboard"] = json!({"available":clipboard.is_some(),"mime_type":"text/plain;charset=utf-8","characters":clipboard.as_ref().map(|s|s.chars().count()),"text_preview":clipboard.as_ref().map(|s|s.chars().take(256).collect::<String>())});
        system["workspace"] =
            json!({"available":active_workspace.is_some(),"active_workspace":active_workspace});
        system["taskbar"] = json!({"available":true,"mode":taskbar_mode});
        system["panels"] = json!({"control":panels.control,"keyboard":panels.keyboard,"notifications":panels.notifications,"clipboard":panels.clipboard,"details":panels.details,"wifi":panels.wifi,"keyboard_page":match panels.keyboard_page {scene::KeyboardPage::Letters => "letters",scene::KeyboardPage::Symbols => "symbols",scene::KeyboardPage::Chinese => "chinese"}});
        system["wifi_ui"] = json!({"busy":wifi.busy,"selected":wifi.selected.as_ref().map(|ap|&ap.id),"editing":wifi.editing,"message":wifi.message});
        if system != last_system {
            client.send("SystemState", system.clone(), &[])?;
            write_state(&o.state_file, &system)?;
            last_system = system;
            scene_dirty = true;
        }
        if scene_dirty && !pending {
            let built = scene::build(
                &last_system,
                &notices,
                active_workspace,
                clipboard.as_deref(),
                &last_result,
                &panels,
                &wifi,
            );
            let mut ops = built.operations();
            let fresh: BTreeSet<String> = built.widgets.iter().map(|w| w.id.clone()).collect();
            for old in node_ids.difference(&fresh) {
                ops.push(json!({"op":"remove_node","node_id":old}));
            }
            let model = json!(ops);
            if model != sent_scene {
                widgets = built
                    .widgets
                    .into_iter()
                    .map(|mut widget| {
                        if interactions.get(&widget.id).is_some_and(|s| s.pressed) {
                            if let Some(old) = widgets.get(&widget.id) {
                                if widget.kind == Kind::Slider {
                                    widget.value = old.value;
                                }
                            }
                        }
                        (widget.id.clone(), widget)
                    })
                    .collect();
                interactions.retain(|id, _| widgets.contains_key(id));
                client.send("SceneTransaction",json!({"base_revision":revision.to_string(),"revision":(revision+1).to_string(),"operations":model}),&[])?;
                pending = true;
                sent_scene = model;
                node_ids = fresh;
            }
            scene_dirty = false;
        }
        if let Some(packet) = client.recv(30)? {
            let m = packet.message;
            let payload = &m["payload"];
            match m["type"].as_str().unwrap_or("") {
                "SceneApplied" => {
                    revision = payload["revision"]
                        .as_str()
                        .context("scene revision missing")?
                        .parse()?;
                    pending = false;
                }
                "SceneRejected" => {
                    revision = payload["revision"].as_str().unwrap_or("0").parse()?;
                    pending = false;
                    sent_scene = Value::Null;
                    scene_dirty = true;
                    last_result = format!("场景未接受：{payload}");
                }
                "TaskbarApplied" => {
                    if taskbar_request.as_deref() == m["request_id"].as_str() {
                        let mode = payload["mode"]
                            .as_str()
                            .context("taskbar reply missing mode")?;
                        ensure!(["fixed", "follow"].contains(&mode), "invalid taskbar reply");
                        taskbar_mode = mode.into();
                        save_taskbar_mode(mode)?;
                        taskbar_request = None;
                        scene_dirty = true;
                    }
                }
                "Snapshot" | "StateChanged" => {
                    if let Some(mode) = payload["taskbar_mode"].as_str() {
                        if taskbar_mode != mode && taskbar_request.is_none() {
                            taskbar_mode = mode.into();
                            save_taskbar_mode(mode)?;
                            scene_dirty = true;
                        }
                    }
                    let new = payload["active_workspace"].as_u64();
                    if new != active_workspace {
                        active_workspace = new;
                        scene_dirty = true;
                    }
                    // Snapshot clipboard is bounded preview metadata. Only
                    // ClipboardChanged carries an authoritative full selection.
                }
                "ClipboardChanged" => {
                    clipboard = payload["text"].as_str().map(str::to_owned);
                    last_result = if clipboard.is_some() {
                        "已收到真实Wayland剪贴板更新"
                    } else {
                        "当前隔离剪贴板为空"
                    }
                    .into();
                    scene_dirty = true;
                }
                "SessionEnding" => {
                    println!("XR_SYSTEM_UI_SESSION_ENDING={payload}");
                    break;
                }
                "ComponentCommand" => {
                    let panel = payload["panel"]
                        .as_str()
                        .context("component command missing panel")?;
                    let intent = payload["intent"]
                        .as_str()
                        .context("component command missing intent")?;
                    if panels.command(panel, intent) {
                        if panel == "wifi" {
                            if !panels.wifi {
                                wifi.back();
                            } else {
                                wifi_action("open", &mut wifi, &mut panels, &state, &requests)?;
                            }
                        }
                        println!("XR_SYSTEM_PANEL_COMMAND={payload}");
                        scene_dirty = true;
                    } else {
                        eprintln!("XR_SYSTEM_PANEL_COMMAND_UNSUPPORTED={payload}");
                    }
                }
                "InputAccepted" | "WorkspaceApplied" | "IntentAccepted" => {
                    let id = m["request_id"].as_str().unwrap_or("");
                    if let Some(action) = requests_by_id.remove(id) {
                        last_result = format!("已路由：{action}");
                        scene_dirty = true;
                    }
                }
                "Error" => {
                    last_result = format!("Shell操作失败：{payload}");
                    eprintln!("XR_SYSTEM_SHELL_ERROR={payload}");
                    scene_dirty = true;
                }
                "InputEvent" | "Cancel" => {
                    let Some((action, slider_value)) = input_action(
                        &mut widgets,
                        &mut interactions,
                        payload,
                        m["type"] == "Cancel",
                    ) else {
                        continue;
                    };
                    let action = action.as_str();
                    if let Some(intent) = action.strip_prefix("wifi:") {
                        wifi_action(intent, &mut wifi, &mut panels, &state, &requests)?;
                        scene_dirty = true;
                    } else if let Some(panel) = action.strip_prefix("ui:") {
                        if panels.command(panel, "toggle") {
                            scene_dirty = true;
                        }
                    } else if let Some(page) = action.strip_prefix("keyboard:") {
                        panels.keyboard_page = match page {
                            "letters" => scene::KeyboardPage::Letters,
                            "symbols" => scene::KeyboardPage::Symbols,
                            "chinese" => scene::KeyboardPage::Chinese,
                            _ => bail!("unknown keyboard page"),
                        };
                        scene_dirty = true;
                    } else if let Some(mode) = action.strip_prefix("taskbar:") {
                        ensure!(
                            ["fixed", "follow"].contains(&mode),
                            "invalid taskbar action"
                        );
                        taskbar_request =
                            Some(client.send("TaskbarIntent", json!({"mode":mode}), &[])?);
                    } else if action == "launcher-toggle" {
                        let id = client.send(
                            "ComponentIntent",
                            json!({"role":"dock","panel":"launcher","intent":"toggle"}),
                            &[],
                        )?;
                        requests_by_id.insert(id, action.into());
                    } else if let Some(shortcut) = action.strip_prefix("shortcut:") {
                        let key = match shortcut {
                            "copy" => 46,
                            "paste" => 47,
                            _ => bail!("unknown clipboard shortcut"),
                        };
                        for (keycode, pressed) in
                            [(29, true), (key, true), (key, false), (29, false)]
                        {
                            let id = client.send(
                                "KeyIntent",
                                json!({"keycode":keycode,"pressed":pressed}),
                                &[],
                            )?;
                            requests_by_id.insert(id, action.into());
                        }
                    } else if action == "refresh" {
                        requests.send(("refresh".into(), None))?;
                    } else if let Some(value) = action.strip_prefix("theme:") {
                        requests.send(("theme".into(), Some(value.into())))?;
                    } else if let Some(value) = action.strip_prefix("ime:") {
                        requests.send(("ime".into(), Some(value.into())))?;
                    } else if action == "volume" {
                        let value =
                            slider_value.context("slider gesture missing captured value")?;
                        requests
                            .send(("volume".into(), Some(value.clamp(0., 100.).to_string())))?;
                    } else if action == "mute-toggle" {
                        let muted = state["volume"]["state"]["muted"]
                            .as_bool()
                            .context("mute state unavailable")?;
                        requests.send(("mute".into(), Some((!muted).to_string())))?;
                    } else if let Some(n) = action.strip_prefix("workspace:") {
                        let id = client.send(
                            "WorkspaceIntent",
                            json!({"workspace":n.parse::<u64>()?}),
                            &[],
                        )?;
                        requests_by_id.insert(id, action.into());
                    } else if action == "clipboard-copy" {
                        let id=client.send("ClipboardSet",json!({"mime_type":"text/plain;charset=utf-8","text":"UKUI XR 复制测试"}),&[])?;
                        requests_by_id.insert(id, action.into());
                    } else if action == "clipboard-paste" {
                        if let Some(text) = &clipboard {
                            let id = client.send("TextCommit", json!({"text":text}), &[])?;
                            requests_by_id.insert(id, action.into());
                        }
                    } else if action == "chinese-commit" {
                        let id =
                            client.send("TextCommit", json!({"text":"你好，openKylin XR"}), &[])?;
                        requests_by_id.insert(id, action.into());
                    } else if let Some(text) = action.strip_prefix("type:") {
                        let id = client.send("TextCommit", json!({"text":text}), &[])?;
                        requests_by_id.insert(id, action.into());
                    } else if let Some(code) = action.strip_prefix("key:") {
                        let keys = code
                            .split('+')
                            .map(str::parse::<u32>)
                            .collect::<Result<Vec<_>, _>>()?;
                        ensure!(keys.iter().all(|key| *key <= 767), "invalid evdev keycode");
                        for key in &keys {
                            let id = client.send(
                                "KeyIntent",
                                json!({"keycode":key,"pressed":true}),
                                &[],
                            )?;
                            requests_by_id.insert(id, action.into());
                        }
                        for key in keys.iter().rev() {
                            let id = client.send(
                                "KeyIntent",
                                json!({"keycode":key,"pressed":false}),
                                &[],
                            )?;
                            requests_by_id.insert(id, action.into());
                        }
                    } else if let Some(id) = action.strip_prefix("notice-close:") {
                        if let Some(s) = server {
                            s.dismiss(id.parse()?, 2).await?;
                            scene_dirty = true;
                        }
                    } else if let Some(ids) = action.strip_prefix("notice-action:") {
                        if let Some(s) = server {
                            let (id, index) =
                                ids.split_once(':').context("invalid notice action")?;
                            s.invoke(id.parse()?, index.parse()?).await?;
                            scene_dirty = true;
                        }
                    }
                }
                _ => {}
            }
        }
        // Notice changes are not generated by OS-probe workers.
        let now_notices = server.map(|s| s.store.snapshot()).unwrap_or_default();
        if serde_json::to_value(
            &now_notices
                .into_iter()
                .rev()
                .take(2)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>(),
        )? != serde_json::to_value(&notices)?
        {
            scene_dirty = true;
        }
    }
    Ok(())
}

fn wifi_action(
    intent: &str,
    wifi: &mut wifi::Wifi,
    panels: &mut scene::Panels,
    state: &Value,
    requests: &Requests,
) -> Result<()> {
    match intent {
        "open" => {
            panels.wifi = true;
            if wifi.busy.is_none()
                && state["network"]["state"]["wireless_enabled"] == true
                && state["network"]["state"]["wifi_usable"] == true
            {
                requests.wifi(network::Request::Scan)?;
                wifi.busy = Some("scan");
                wifi.message = "正在扫描附近网络…".into();
            }
        }
        "back" => {
            if wifi.selected.is_some() {
                wifi.back();
            } else {
                panels.wifi = false;
                panels.control = true;
                wifi.clear_secret();
            }
        }
        _ if wifi.busy.is_some() => {}
        "prev" => {
            wifi.page = wifi.page.saturating_sub(1);
        }
        "next" => {
            wifi.page = (wifi.page + 1).min(
                wifi::access_points(state)
                    .len()
                    .div_ceil(4)
                    .saturating_sub(1),
            );
        }
        "edit" => {
            wifi.editing = !wifi.editing;
            wifi.revealed = false;
        }
        "reveal" => {
            wifi.revealed = !wifi.revealed;
        }
        "shift" => {
            wifi.uppercase = !wifi.uppercase;
        }
        "symbols" => {
            wifi.symbols = !wifi.symbols;
        }
        "delete" => {
            wifi.password.pop();
        }
        "done" => {
            wifi.editing = false;
            wifi.revealed = false;
        }
        "scan" => {
            requests.wifi(network::Request::Scan)?;
            wifi.busy = Some("scan");
            wifi.message = "正在扫描附近网络…".into();
        }
        "radio" => {
            let enabled = state["network"]["state"]["wireless_enabled"] == true;
            requests.wifi(network::Request::Radio(!enabled))?;
            wifi.busy = Some("radio");
            wifi.message = "正在更新无线开关…".into();
        }
        "disconnect" => {
            if let Some(ap) = wifi.selected.clone() {
                requests.wifi(network::Request::Disconnect(ap))?;
                wifi.busy = Some("disconnect");
                wifi.message = "正在断开连接…".into();
            }
        }
        "connect" => {
            if let Some(ap) = wifi.selected.clone() {
                if network::valid_password(&ap, &wifi.password) {
                    requests.wifi(network::Request::Connect {
                        ap,
                        password: wifi.password.clone(),
                    })?;
                    wifi.busy = Some("connect");
                    wifi.message = "正在连接，请稍候…".into();
                    wifi.revealed = false;
                    wifi.editing = false;
                }
            }
        }
        _ => {
            if let Some(id) = intent.strip_prefix("select:") {
                if let Some(ap) = wifi::access_points(state)
                    .into_iter()
                    .find(|ap| ap.id == id)
                {
                    wifi.back();
                    wifi.selected = Some(ap);
                }
            } else if let Some(value) = intent.strip_prefix("char:") {
                wifi.append(value);
            }
        }
    }
    Ok(())
}
