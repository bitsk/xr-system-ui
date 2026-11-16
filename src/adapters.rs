// SPDX-License-Identifier: GPL-3.0-or-later
//! Real command/service adapters. These run on a worker, never a render thread.
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::process::Command;

pub fn command(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new("timeout")
        .arg("3")
        .arg(program)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .with_context(|| format!("run {program}"))?;
    ensure!(
        out.status.success(),
        "{program}: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8(out.stdout)?.trim().to_owned())
}
fn quoted(raw: String) -> String {
    raw.trim().trim_matches('\'').trim_matches('"').to_string()
}
fn setting(schema: &str, key: &str) -> Result<String> {
    Ok(quoted(command("gsettings", &["get", schema, key])?))
}
fn capability(result: Result<Value>) -> Value {
    match result {
        Ok(state) => json!({"available":true,"state":state}),
        Err(e) => json!({"available":false,"error":format!("{e:#}")}),
    }
}

pub fn theme() -> Result<Value> {
    let name = setting("org.ukui.style", "style-name")?;
    Ok(json!({"backend":"GSettings org.ukui.style","name":name,
        "font":setting("org.ukui.style","system-font")?,
        "font_size":setting("org.ukui.style","system-font-size")?,
        "accent":setting("org.ukui.style","theme-color")?,
        "icon_theme":setting("org.ukui.style","icon-theme-name")?}))
}
pub fn volume() -> Result<Value> {
    let default = command("pactl", &["get-default-sink"])?;
    let sinks: Value = serde_json::from_str(&command("pactl", &["-f", "json", "list", "sinks"])?)?;
    let sink = sinks
        .as_array()
        .context("Pulse sink list is not an array")?
        .iter()
        .find(|s| s["name"].as_str() == Some(default.as_str()))
        .context("default sink is absent")?;
    let channels = sink["volume"]
        .as_object()
        .context("sink volume channels absent")?;
    ensure!(!channels.is_empty(), "sink has no volume channels");
    let values = channels
        .values()
        .map(|v| v["value"].as_u64().context("missing channel volume"))
        .collect::<Result<Vec<_>>>()?;
    let percent = values
        .iter()
        .map(|v| *v as f64 / 65536.0 * 100.0)
        .sum::<f64>()
        / values.len() as f64;
    let virtual_sink = sink["properties"]["node.virtual"].as_str() == Some("true")
        || sink["properties"]["node.virtual"].as_bool() == Some(true)
        || default == "auto_null";
    Ok(
        json!({"backend":"Pulse API","sink_name":default,"description":sink["description"],
        "percent":percent,"muted":sink["mute"].as_bool().context("missing mute state")?,
        "virtual_sink":virtual_sink,"hardware_audio_verified":false}),
    )
}
pub fn network() -> Result<Value> {
    crate::network::snapshot()
}
#[derive(serde::Serialize)]
struct BatteryReading {
    name: String,
    present: Option<bool>,
    capacity_percent: Option<f64>,
    status: String,
    energy_now_uwh: Option<f64>,
    energy_full_uwh: Option<f64>,
}
fn supply_text(path: &std::path::Path, attribute: &str) -> Option<String> {
    std::fs::read_to_string(path.join(attribute))
        .ok()
        .map(|s| s.trim().to_owned())
}
fn supply_number(path: &std::path::Path, attribute: &str) -> Option<f64> {
    supply_text(path, attribute)?
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && *n >= 0.)
}
fn energy_percent(now: f64, full: f64) -> Option<f64> {
    if !now.is_finite() || !full.is_finite() || full <= 0. {
        return None;
    }
    let percent = now / full * 100.;
    percent.is_finite().then(|| percent.clamp(0., 100.))
}
/// Read Linux power_supply attributes only; never write charging policy or
/// combine unlike charge/energy units or assume equally sized battery packs.
pub fn battery() -> Result<Value> {
    let root = std::path::Path::new("/sys/class/power_supply");
    let mut batteries = Vec::new();
    for entry in std::fs::read_dir(root).context("read power_supply class")? {
        let path = entry?.path();
        if supply_text(&path, "type").as_deref() != Some("Battery")
            || supply_text(&path, "scope").as_deref() == Some("Device")
        {
            continue;
        }
        let present = match supply_text(&path, "present").as_deref() {
            Some("0") => Some(false),
            Some("1") => Some(true),
            _ => None,
        };
        if present == Some(false) {
            continue;
        }
        batteries.push(BatteryReading {
            name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            present,
            capacity_percent: supply_number(&path, "capacity").filter(|n| *n <= 100.),
            status: supply_text(&path, "status").unwrap_or_else(|| "Unknown".into()),
            energy_now_uwh: supply_number(&path, "energy_now"),
            energy_full_uwh: supply_number(&path, "energy_full").filter(|n| *n > 0.),
        });
    }
    ensure!(
        !batteries.is_empty(),
        "no present system battery reported by power_supply"
    );
    batteries.sort_by(|a, b| a.name.cmp(&b.name));
    let (percent, aggregation) = if batteries.len() == 1 {
        let b = &batteries[0];
        if let Some(capacity) = b.capacity_percent {
            (Some(capacity), "reported_capacity")
        } else {
            (
                b.energy_now_uwh
                    .zip(b.energy_full_uwh)
                    .and_then(|(n, f)| energy_percent(n, f)),
                "energy_ratio",
            )
        }
    } else if batteries
        .iter()
        .all(|b| b.energy_now_uwh.is_some() && b.energy_full_uwh.is_some())
    {
        let now: f64 = batteries.iter().filter_map(|b| b.energy_now_uwh).sum();
        let full: f64 = batteries.iter().filter_map(|b| b.energy_full_uwh).sum();
        (energy_percent(now, full), "energy_totals")
    } else {
        (None, "unavailable_without_comparable_energy")
    };
    let charging = batteries.iter().any(|b| b.status == "Charging");
    let discharging = batteries.iter().any(|b| b.status == "Discharging");
    let known = batteries
        .iter()
        .all(|b| ["Charging", "Discharging", "Full", "Not charging"].contains(&b.status.as_str()));
    let status = if !known {
        "Unknown"
    } else if charging && discharging {
        "Mixed"
    } else if charging {
        "Charging"
    } else if discharging {
        "Discharging"
    } else if batteries.iter().all(|b| b.status == "Full") {
        "Full"
    } else {
        "Not charging"
    };
    Ok(
        json!({"backend":"Linux power_supply sysfs (read only)","percent":percent,
        "percent_known":percent.is_some(),"status":status,"aggregation":aggregation,
        "present_count":batteries.len(),"batteries":batteries}),
    )
}

pub fn ime() -> Result<Value> {
    let owner = running_ime_owner()?;
    // Use the unique connection name, so a daemon exiting between NameHasOwner
    // and this query cannot turn a read-only probe into D-Bus activation.
    let current = tuple_string(command(
        "gdbus",
        &[
            "call",
            "--session",
            "--dest",
            &owner,
            "--object-path",
            "/controller",
            "--method",
            "org.fcitx.Fcitx.Controller1.CurrentInputMethod",
        ],
    )?)?;
    ensure!(
        !current.is_empty(),
        "Fcitx has no current input method/context"
    );
    let methods = command(
        "gdbus",
        &[
            "call",
            "--session",
            "--dest",
            &owner,
            "--object-path",
            "/controller",
            "--method",
            "org.fcitx.Fcitx.Controller1.AvailableInputMethods",
        ],
    )?;
    Ok(
        json!({"backend":"Fcitx5 Controller1","current":current,"owner_unique_name":owner,
        "pinyin_available":methods.contains("'pinyin'"),
        "text_input_owner":"Wayland seat/text-input/input-method service",
        "client_preedit_verified":false}),
    )
}
fn tuple_string(raw: String) -> Result<String> {
    let inner = raw
        .trim()
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .context("invalid D-Bus string tuple")?
        .trim()
        .trim_end_matches(',')
        .trim();
    ensure!(
        inner.len() >= 2
            && ((inner.starts_with('\'') && inner.ends_with('\''))
                || (inner.starts_with('"') && inner.ends_with('"'))),
        "expected D-Bus string result"
    );
    Ok(inner[1..inner.len() - 1].to_owned())
}
fn running_ime_owner() -> Result<String> {
    let has = command(
        "gdbus",
        &[
            "call",
            "--session",
            "--dest",
            "org.freedesktop.DBus",
            "--object-path",
            "/org/freedesktop/DBus",
            "--method",
            "org.freedesktop.DBus.NameHasOwner",
            "org.fcitx.Fcitx5",
        ],
    )?;
    ensure!(
        has.trim() == "(true,)",
        "Fcitx5 is not running on this session bus; start it through the session manager"
    );
    let owner = tuple_string(command(
        "gdbus",
        &[
            "call",
            "--session",
            "--dest",
            "org.freedesktop.DBus",
            "--object-path",
            "/org/freedesktop/DBus",
            "--method",
            "org.freedesktop.DBus.GetNameOwner",
            "org.fcitx.Fcitx5",
        ],
    )?)?;
    ensure!(owner.starts_with(':'), "invalid Fcitx5 unique bus owner");
    Ok(owner)
}
pub fn notification_owner() -> Result<Value> {
    let has = command(
        "gdbus",
        &[
            "call",
            "--session",
            "--dest",
            "org.freedesktop.DBus",
            "--object-path",
            "/org/freedesktop/DBus",
            "--method",
            "org.freedesktop.DBus.NameHasOwner",
            "org.freedesktop.Notifications",
        ],
    )?;
    if has.contains("true") {
        let owner = command(
            "gdbus",
            &[
                "call",
                "--session",
                "--dest",
                "org.freedesktop.DBus",
                "--object-path",
                "/org/freedesktop/DBus",
                "--method",
                "org.freedesktop.DBus.GetNameOwner",
                "org.freedesktop.Notifications",
            ],
        )?;
        Ok(json!({"owner":owner,"claim_requested":false}))
    } else {
        Ok(json!({"owner":null,"claim_requested":false}))
    }
}
fn xr_command(args: &[&str]) -> Result<Value> {
    let configured = std::env::var("XR_SYSTEM_CONTROLS_COMMAND").context(
        "XR controls are owned by the active runtime; no system controls provider configured",
    )?;
    let provider: Vec<String> = serde_json::from_str(&configured)
        .context("XR_SYSTEM_CONTROLS_COMMAND must be a JSON argv array")?;
    ensure!(
        !provider.is_empty() && std::path::Path::new(&provider[0]).is_absolute(),
        "XR controls provider requires an absolute executable path"
    );
    let out = Command::new("timeout")
        .arg("30")
        .args(&provider)
        .args(args)
        .output()
        .context("connect XR session controls")?;
    ensure!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(serde_json::from_slice(&out.stdout).context("decode XR controls response")?)
}
fn xr_controls() -> Result<Value> {
    let mut state = xr_command(&["get"])?;
    ensure!(
        state["session_active"] == true,
        "XR controls are not active"
    );
    for (key, variable) in [
        ("runtime", "XR_INPUT_STATUS_FILE"),
        ("scene", "XR_SCENE_CONTROLS_STATUS_FILE"),
    ] {
        if let Some(path) = std::env::var_os(variable) {
            let path = std::path::PathBuf::from(path);
            if !std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age < std::time::Duration::from_secs(4))
            {
                continue;
            }
            if let Ok(bytes) = std::fs::read(path) {
                if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                    state[key] = value;
                }
            }
        }
    }
    Ok(state)
}
pub fn probe() -> Value {
    json!({"xr":capability(xr_controls()),"theme":capability(theme()),"volume":capability(volume()),
        "network":capability(network()),"battery":capability(battery()),"ime":capability(ime()),
        "notifications":capability(notification_owner()),
        "clipboard":{"available":false,"reason":"requires connected private Shell/Wayland selection bridge"},
        "workspace":{"available":false,"reason":"requires Shell snapshot"}})
}

pub fn action(name: &str, value: Option<&str>) -> Result<Value> {
    match name {
        "xr-input" | "xr-hand-tracking" | "xr-passthrough" | "xr-double-tap" => {
            let field = match name {
                "xr-input" => "input_mode",
                "xr-hand-tracking" => "hand_tracking_enabled",
                "xr-passthrough" => "passthrough_enabled",
                _ => "double_tap_enabled",
            };
            let value = value.context("XR control requires a value")?;
            let observed = xr_command(&["set", field, value])?;
            Ok(json!({"action":name,"observed":observed}))
        }
        "theme" => {
            let target = value.context("theme requires ukui-light or ukui-dark")?;
            ensure!(
                ["ukui-light", "ukui-dark"].contains(&target),
                "unsupported UKUI theme"
            );
            command(
                "gsettings",
                &["set", "org.ukui.style", "style-name", target],
            )?;
            let observed = theme()?;
            ensure!(observed["name"] == target, "theme readback mismatch");
            Ok(json!({"action":"theme","requested":target,"observed":observed}))
        }
        "volume" => {
            let percent: f64 = value.context("volume requires 0..100")?.parse()?;
            ensure!(
                percent.is_finite() && (0.0..=100.0).contains(&percent),
                "volume outside 0..100"
            );
            command(
                "pactl",
                &["set-sink-volume", "@DEFAULT_SINK@", &format!("{percent}%")],
            )?;
            let observed = volume()?;
            ensure!(
                (observed["percent"].as_f64().context("no observed volume")? - percent).abs() < 0.1,
                "volume readback mismatch"
            );
            Ok(json!({"action":"volume","requested":percent,"observed":observed}))
        }
        "mute" => {
            let target = value.context("mute requires true or false")?;
            ensure!(
                ["true", "false"].contains(&target),
                "mute requires true or false"
            );
            command(
                "pactl",
                &[
                    "set-sink-mute",
                    "@DEFAULT_SINK@",
                    if target == "true" { "1" } else { "0" },
                ],
            )?;
            let observed = volume()?;
            ensure!(
                observed["muted"] == (target == "true"),
                "mute readback mismatch"
            );
            Ok(json!({"action":"mute","requested":target,"observed":observed}))
        }
        "ime" => {
            let target = value.context("ime requires pinyin or keyboard-us")?;
            ensure!(
                ["pinyin", "keyboard-us"].contains(&target),
                "unsupported input method"
            );
            running_ime_owner()?;
            command("fcitx5-remote", &["-s", target])?;
            if target == "pinyin" {
                command("fcitx5-remote", &["-o"])?;
            }
            let mut observed = ime()?;
            for _ in 0..5 {
                if observed["current"] == target {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
                observed = ime()?;
            }
            ensure!(
                observed["current"] == target,
                "Fcitx input method readback mismatch: {observed}"
            );
            Ok(json!({"action":"ime","requested":target,"observed":observed}))
        }
        "refresh" => Ok(json!({"action":"refresh","observed":probe()})),
        _ => bail!("unknown system action {name}"),
    }
}
