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
    let general = command("nmcli", &["-t", "-f", "STATE,CONNECTIVITY", "general"])?;
    let (state, connectivity) = general
        .split_once(':')
        .context("unexpected NetworkManager status")?;
    let mut devices = Vec::new();
    for line in command(
        "nmcli",
        &["-t", "-f", "DEVICE,TYPE,STATE", "device", "status"],
    )?
    .lines()
    {
        let mut parts = line.splitn(3, ':');
        if let (Some(name), Some(kind), Some(status)) = (parts.next(), parts.next(), parts.next()) {
            devices.push(json!({"name":name,"kind":kind,"state":status}));
        }
    }
    let has_wifi = devices.iter().any(|d| d["kind"] == "wifi");
    Ok(
        json!({"backend":"NetworkManager","state":state,"connectivity":connectivity,
        "devices":devices,"wifi_device_available":has_wifi,"control":"status_only"}),
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
pub fn probe() -> Value {
    json!({"theme":capability(theme()),"volume":capability(volume()),
        "network":capability(network()),"ime":capability(ime()),
        "notifications":capability(notification_owner()),
        "clipboard":{"available":false,"reason":"requires connected private Shell/Wayland selection bridge"},
        "workspace":{"available":false,"reason":"requires Shell snapshot"}})
}

pub fn action(name: &str, value: Option<&str>) -> Result<Value> {
    match name {
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
