// SPDX-License-Identifier: GPL-3.0-or-later
//! NetworkManager work runs on the system worker. Secrets never enter scene/state JSON.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, future::Future, time::Duration};
use zbus::{
    Connection, Proxy,
    zvariant::{OwnedObjectPath, OwnedValue, Value as Variant},
};

const NM: &str = "org.freedesktop.NetworkManager";
const ROOT: &str = "/org/freedesktop/NetworkManager";
type Properties = HashMap<String, OwnedValue>;
type Settings = HashMap<String, Properties>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccessPoint {
    pub id: String,
    pub path: String,
    pub device: String,
    pub interface: String,
    pub ssid: String,
    pub ssid_bytes: Vec<u8>,
    pub strength: u8,
    pub security: String,
    pub key_mgmt: String,
    pub connected: bool,
    pub connectivity: String,
    pub saved: Option<String>,
}
impl AccessPoint {
    pub fn needs_password(&self) -> bool {
        ["wpa-psk", "sae"].contains(&self.key_mgmt.as_str())
    }
    pub fn supported(&self) -> bool {
        self.saved.is_some() || ["", "wpa-psk", "sae", "owe"].contains(&self.key_mgmt.as_str())
    }
}

// Deliberately no Debug/Serialize: a password has no telemetry or argv representation.
pub enum Request {
    Scan,
    Radio(bool),
    Connect { ap: AccessPoint, password: String },
    Disconnect(AccessPoint),
}
impl Request {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Scan => "scan",
            Self::Radio(_) => "radio",
            Self::Connect { .. } => "connect",
            Self::Disconnect(_) => "disconnect",
        }
    }
}
fn run<T: Send>(seconds: u64, future: impl Future<Output = Result<T>> + Send) -> Result<T> {
    // probe/action CLI entry points also run inside Tokio; do not nest runtimes.
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?
                    .block_on(async {
                        tokio::time::timeout(Duration::from_secs(seconds), future)
                            .await
                            .context("网络操作超时，请重试")?
                    })
            })
            .join()
            .map_err(|_| anyhow::anyhow!("网络工作线程退出"))?
    })
}
async fn proxy<'a>(bus: &'a Connection, path: &'a str, interface: &str) -> Result<Proxy<'a>> {
    Ok(Proxy::new(bus, NM, path, interface.to_owned()).await?)
}
async fn properties(bus: &Connection, path: &str, interface: &str) -> Result<Properties> {
    Ok(proxy(bus, path, "org.freedesktop.DBus.Properties")
        .await?
        .call("GetAll", &(interface,))
        .await?)
}
fn uint(p: &Properties, key: &str) -> u32 {
    p.get(key).and_then(|v| u32::try_from(v).ok()).unwrap_or(0)
}
fn boolean(p: &Properties, key: &str) -> bool {
    p.get(key)
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(false)
}
fn string(p: &Properties, key: &str) -> String {
    p.get(key)
        .and_then(|v| <&str>::try_from(v).ok())
        .unwrap_or("")
        .into()
}
fn paths(p: &mut Properties, key: &str) -> Vec<OwnedObjectPath> {
    p.remove(key)
        .and_then(|v| Vec::try_from(v).ok())
        .unwrap_or_default()
}
fn path(p: &Properties, key: &str) -> String {
    p.get(key)
        .and_then(|v| <&zbus::zvariant::ObjectPath>::try_from(v).ok())
        .map(|p| p.to_string())
        .unwrap_or_else(|| "/".into())
}
fn bytes(p: &mut Properties, key: &str) -> Vec<u8> {
    p.remove(key)
        .and_then(|v| Vec::try_from(v).ok())
        .unwrap_or_default()
}
fn security(flags: u32, wpa: u32, rsn: u32) -> (&'static str, &'static str) {
    if rsn & 0x400 != 0 {
        ("WPA3 个人", "sae")
    } else if (wpa | rsn) & 0x100 != 0 {
        ("WPA/WPA2 个人", "wpa-psk")
    } else if rsn & 0x1800 != 0 {
        ("增强开放网络", "owe")
    } else if (wpa | rsn) & 0x2200 != 0 {
        ("企业认证", "wpa-eap")
    } else if flags & 1 != 0 {
        ("旧式或不支持的加密", "unsupported")
    } else {
        ("开放网络", "")
    }
}
fn state_name(state: u32) -> &'static str {
    match state {
        100 => "connected",
        40..=90 => "connecting",
        30 => "disconnected",
        20 => "unavailable",
        10 => "unmanaged",
        _ => "unknown",
    }
}
fn connectivity_name(state: u32) -> &'static str {
    match state {
        4 => "full",
        3 => "limited",
        2 => "portal",
        1 => "none",
        _ => "unknown",
    }
}
fn deduplicate(aps: Vec<AccessPoint>) -> Vec<AccessPoint> {
    let mut unique = HashMap::<(String, Vec<u8>, String), AccessPoint>::new();
    for ap in aps {
        let key = (
            ap.device.clone(),
            ap.ssid_bytes.clone(),
            ap.key_mgmt.clone(),
        );
        if unique
            .get(&key)
            .is_none_or(|old| (ap.connected, ap.strength) > (old.connected, old.strength))
        {
            unique.insert(key, ap);
        }
    }
    let mut aps: Vec<_> = unique.into_values().collect();
    aps.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then(b.strength.cmp(&a.strength))
            .then(a.ssid.cmp(&b.ssid))
            .then(a.id.cmp(&b.id))
    });
    aps
}

pub fn snapshot() -> Result<Value> {
    run(8, async { snapshot_on(&Connection::system().await?).await })
}
async fn snapshot_on(bus: &Connection) -> Result<Value> {
    let mut general = properties(bus, ROOT, NM).await?;
    let mut devices = Vec::new();
    let mut aps = Vec::new();
    let mut scan_errors = false;
    let mut wifi_usable = false;
    for device in paths(&mut general, "Devices") {
        let mut d = properties(bus, device.as_str(), &format!("{NM}.Device")).await?;
        let kind = match uint(&d, "DeviceType") {
            1 => "ethernet",
            2 => "wifi",
            32 => "loopback",
            _ => "other",
        };
        let interface = string(&d, "Interface");
        let state = uint(&d, "State");
        devices.push(json!({"name":interface,"kind":kind,"state":state_name(state)}));
        if kind != "wifi" {
            continue;
        }
        wifi_usable |= state >= 30;
        let wireless =
            match properties(bus, device.as_str(), &format!("{NM}.Device.Wireless")).await {
                Ok(p) => p,
                Err(_) => {
                    scan_errors = true;
                    continue;
                }
            };
        let active = path(&wireless, "ActiveAccessPoint");
        let active_connection = path(&d, "ActiveConnection");
        let active_profile = if active_connection != "/" {
            properties(bus, &active_connection, &format!("{NM}.Connection.Active"))
                .await
                .ok()
                .map(|p| path(&p, "Connection"))
        } else {
            None
        };
        let mut profiles = Vec::new();
        for profile in paths(&mut d, "AvailableConnections") {
            // GetSettings excludes secrets. Profile names are never mistaken for SSIDs.
            let Ok(p) = proxy(bus, profile.as_str(), &format!("{NM}.Settings.Connection")).await
            else {
                continue;
            };
            let Ok(mut settings) = p.call::<_, _, Settings>("GetSettings", &()).await else {
                continue;
            };
            let ssid = settings
                .get_mut("802-11-wireless")
                .map(|p| bytes(p, "ssid"))
                .unwrap_or_default();
            let mgmt = settings
                .get("802-11-wireless-security")
                .map(|p| string(p, "key-mgmt"))
                .unwrap_or_default();
            profiles.push((ssid, mgmt, profile.to_string()));
        }
        profiles.sort_by_key(|(_, _, p)| active_profile.as_ref() != Some(p));
        let w = proxy(bus, device.as_str(), &format!("{NM}.Device.Wireless")).await?;
        let points: Vec<OwnedObjectPath> = match w.call("GetAllAccessPoints", &()).await {
            Ok(p) => p,
            Err(_) => {
                scan_errors = true;
                continue;
            }
        };
        for point in points {
            let Ok(mut a) = properties(bus, point.as_str(), &format!("{NM}.AccessPoint")).await
            else {
                continue;
            };
            let ssid = bytes(&mut a, "Ssid");
            if ssid.is_empty() {
                continue;
            }
            let (label, mgmt) = security(
                uint(&a, "Flags"),
                uint(&a, "WpaFlags"),
                uint(&a, "RsnFlags"),
            );
            aps.push(AccessPoint {
                id: format!(
                    "{}-{}",
                    device.as_str().rsplit('/').next().unwrap_or("0"),
                    point.as_str().rsplit('/').next().unwrap_or("0")
                ),
                path: point.to_string(),
                device: device.to_string(),
                interface: interface.clone(),
                ssid: String::from_utf8_lossy(&ssid)
                    .chars()
                    .map(|c| if c.is_control() { '�' } else { c })
                    .collect(),
                saved: profiles
                    .iter()
                    .find(|(s, k, _)| *s == ssid && k == mgmt)
                    .map(|(_, _, p)| p.clone()),
                ssid_bytes: ssid,
                security: label.into(),
                key_mgmt: mgmt.into(),
                strength: a
                    .get("Strength")
                    .and_then(|v| u8::try_from(v).ok())
                    .unwrap_or(0)
                    .min(100),
                connected: state == 100 && point.as_str() == active,
                connectivity: connectivity_name(
                    uint(&d, "Ip4Connectivity").max(uint(&d, "Ip6Connectivity")),
                )
                .into(),
            });
        }
    }
    Ok(
        json!({"backend":"NetworkManager D-Bus","state":match uint(&general,"State") { 70=>"connected",60=>"connected (site only)",50=>"connected (local only)",40=>"connecting",_=>"disconnected" },
        "connectivity":connectivity_name(uint(&general,"Connectivity")),
        "wifi_device_available":devices.iter().any(|d|d["kind"]=="wifi"),"wifi_usable":wifi_usable,
        "wireless_enabled":boolean(&general,"WirelessEnabled"),"wireless_hardware_enabled":boolean(&general,"WirelessHardwareEnabled"),
        "devices":devices,"access_points":deduplicate(aps),"scan_error":scan_errors,"control":"wifi"}),
    )
}

pub fn execute(request: Request) -> Result<Value> {
    let name = request.name();
    // Bound the whole D-Bus operation, including activation/scan observation.
    let result = run(40, async {
        let bus = Connection::system().await?;
        let manager = proxy(&bus, ROOT, NM).await?;
        match request {
            Request::Radio(enabled) => {
                manager.set_property("WirelessEnabled", enabled).await?;
                ensure!(
                    manager.get_property::<bool>("WirelessEnabled").await? == enabled,
                    "无线开关未生效"
                );
            }
            Request::Scan => {
                let devices: Vec<OwnedObjectPath> = manager.call("GetDevices", &()).await?;
                let mut count = 0;
                for device in devices {
                    let d = proxy(&bus, device.as_str(), &format!("{NM}.Device")).await?;
                    if d.get_property::<u32>("DeviceType").await? != 2
                        || d.get_property::<u32>("State").await? < 30
                    {
                        continue;
                    }
                    let w = proxy(&bus, device.as_str(), &format!("{NM}.Device.Wireless")).await?;
                    let before: i64 = w.get_property("LastScan").await?;
                    w.call::<_, _, ()>("RequestScan", &(Properties::new(),))
                        .await?;
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
                    loop {
                        if w.get_property::<i64>("LastScan").await? > before {
                            break;
                        }
                        ensure!(tokio::time::Instant::now() < deadline, "扫描超时，请重试");
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                    count += 1;
                }
                ensure!(count > 0, "没有可用的无线设备");
            }
            Request::Disconnect(ap) => {
                proxy(&bus, &ap.device, &format!("{NM}.Device"))
                    .await?
                    .call::<_, _, ()>("Disconnect", &())
                    .await?;
                let d = proxy(&bus, &ap.device, &format!("{NM}.Device")).await?;
                for _ in 0..40 {
                    if d.get_property::<u32>("State").await? <= 30 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                ensure!(
                    d.get_property::<u32>("State").await? <= 30,
                    "断开连接尚未完成"
                );
            }
            Request::Connect { ap, password } => {
                ensure!(ap.supported(), "此网络需要在系统设置中配置认证");
                ensure!(
                    valid_password(&ap, &password),
                    "请检查密码长度或使用已保存的连接"
                );
                let mut actual = properties(&bus, &ap.path, &format!("{NM}.AccessPoint")).await?;
                ensure!(
                    bytes(&mut actual, "Ssid") == ap.ssid_bytes,
                    "网络已变化，请刷新后重选"
                );
                ensure!(
                    security(
                        uint(&actual, "Flags"),
                        uint(&actual, "WpaFlags"),
                        uint(&actual, "RsnFlags")
                    )
                    .1 == ap.key_mgmt,
                    "网络已变化，请刷新后重选"
                );
                let device = OwnedObjectPath::try_from(ap.device.as_str())?;
                let point = OwnedObjectPath::try_from(ap.path.as_str())?;
                let active: OwnedObjectPath;
                let mut created: Option<OwnedObjectPath> = None;
                if password.is_empty() && ap.saved.is_some() {
                    active = manager
                        .call(
                            "ActivateConnection",
                            &(
                                OwnedObjectPath::try_from(ap.saved.as_deref().unwrap())?,
                                &device,
                                &point,
                            ),
                        )
                        .await?;
                } else {
                    let settings = connection_settings(&ap, &password)?;
                    // Save only after activation succeeds; failed credentials leave no disk profile.
                    let options = HashMap::from([("persist", Variant::from("memory"))]);
                    let (profile, connection, _): (OwnedObjectPath, OwnedObjectPath, Properties) =
                        manager
                            .call(
                                "AddAndActivateConnection2",
                                &(settings, &device, &point, options),
                            )
                            .await?;
                    created = Some(profile);
                    active = connection;
                }
                drop(password);
                let activation = async {
                    let a =
                        proxy(&bus, active.as_str(), &format!("{NM}.Connection.Active")).await?;
                    loop {
                        match a.get_property::<u32>("State").await? {
                            2 => break,
                            3 | 4 => bail!("连接失败，请检查密码和路由器后重试"),
                            _ => tokio::time::sleep(Duration::from_millis(250)).await,
                        }
                    }
                    let d = proxy(&bus, &ap.device, &format!("{NM}.Device")).await?;
                    ensure!(
                        d.get_property::<u32>("State").await? == 100
                            && d.get_property::<OwnedObjectPath>("ActiveConnection")
                                .await?
                                == active,
                        "连接状态尚未确认"
                    );
                    Ok(())
                };
                let outcome = tokio::time::timeout(Duration::from_secs(30), activation)
                    .await
                    .context("连接超时，请重试")
                    .and_then(|r| r);
                if outcome.is_err() {
                    let _ = manager
                        .call::<_, _, ()>("DeactivateConnection", &(&active,))
                        .await;
                }
                if let Some(profile) = created {
                    let p =
                        proxy(&bus, profile.as_str(), &format!("{NM}.Settings.Connection")).await?;
                    if outcome.is_ok() {
                        p.call::<_, _, ()>("Save", &()).await?;
                    } else {
                        let _ = p.call::<_, _, ()>("Delete", &()).await;
                    }
                }
                outcome?;
            }
        }
        Ok(json!({"action":name,"observed":snapshot_on(&bus).await?}))
    });
    // Never forward arbitrary service error text: it may contain connection settings.
    result.map_err(|e| {
        let message = e.to_string();
        if message.contains("PermissionDenied")
            || message.contains("NotAuthorized")
            || message.contains("AccessDenied")
        {
            anyhow::anyhow!("当前会话无权修改无线网络，请在系统设置中授权")
        } else if [
            "网络操作超时，请重试",
            "此网络需要在系统设置中配置认证",
            "请检查密码长度或使用已保存的连接",
            "网络已变化，请刷新后重选",
            "连接失败，请检查密码和路由器后重试",
            "连接超时，请重试",
            "连接状态尚未确认",
            "扫描超时，请重试",
            "没有可用的无线设备",
            "无线开关未生效",
            "断开连接尚未完成",
        ]
        .contains(&message.as_str())
        {
            anyhow::anyhow!(message)
        } else {
            anyhow::anyhow!(match name {
                "scan" => "扫描失败，请稍后刷新或检查设备",
                "radio" => "无线开关更新失败，请检查系统授权",
                "disconnect" => "断开连接失败，请重试",
                _ => "连接失败，请检查密码、设备或系统授权",
            })
        }
    })
}
pub fn valid_password(ap: &AccessPoint, password: &str) -> bool {
    if password.is_empty() {
        return !ap.needs_password() || ap.saved.is_some();
    }
    match ap.key_mgmt.as_str() {
        "wpa-psk" => {
            (8..=63).contains(&password.len())
                || (password.len() == 64 && password.bytes().all(|c| c.is_ascii_hexdigit()))
        }
        "sae" => password.len() <= 63,
        _ => false,
    }
}
fn connection_settings<'a>(
    ap: &'a AccessPoint,
    password: &'a str,
) -> Result<HashMap<&'a str, HashMap<&'a str, Variant<'a>>>> {
    let mut settings = HashMap::from([
        (
            "connection",
            HashMap::from([
                ("id", Variant::from(ap.ssid.as_str())),
                ("type", Variant::from("802-11-wireless")),
                ("autoconnect", Variant::from(true)),
            ]),
        ),
        (
            "802-11-wireless",
            HashMap::from([
                ("ssid", Variant::from(ap.ssid_bytes.clone())),
                ("mode", Variant::from("infrastructure")),
            ]),
        ),
    ]);
    if !ap.key_mgmt.is_empty() {
        let mut security = HashMap::from([("key-mgmt", Variant::from(ap.key_mgmt.as_str()))]);
        if ap.needs_password() {
            security.insert("psk", Variant::from(password));
        }
        settings.insert("802-11-wireless-security", security);
    }
    Ok(settings)
}
