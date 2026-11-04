// SPDX-License-Identifier: GPL-3.0-or-later
//! Local Wi-Fi navigation and secure keyboard. Only explicit reveal sends secret glyphs to Shell.
use crate::network::{AccessPoint, valid_password};
use serde_json::Value;
use xr_spatial_kit::{Scene, Widget};

#[derive(Default)]
pub struct Wifi {
    pub selected: Option<AccessPoint>,
    pub password: String,
    pub revealed: bool,
    pub editing: bool,
    pub uppercase: bool,
    pub symbols: bool,
    pub page: usize,
    pub busy: Option<&'static str>,
    pub message: String,
}
impl Wifi {
    pub fn clear_secret(&mut self) {
        self.password.clear();
        self.revealed = false;
        self.editing = false;
        self.uppercase = false;
        self.symbols = false;
    }
    pub fn back(&mut self) {
        self.clear_secret();
        self.selected = None;
        self.message.clear();
    }
    pub fn append(&mut self, value: &str) {
        if self.busy.is_none()
            && self.password.len() + value.len() <= 64
            && !value.chars().any(char::is_control)
        {
            self.password.push_str(value);
        }
    }
}
pub fn access_points(state: &Value) -> Vec<AccessPoint> {
    serde_json::from_value(state["network"]["state"]["access_points"].clone()).unwrap_or_default()
}
pub fn connected_label(ap: &AccessPoint) -> &'static str {
    match ap.connectivity.as_str() {
        "full" => "已连接 · 互联网可用",
        "portal" => "已连接 · 需登录认证",
        "limited" | "none" => "已连接 · 互联网受限",
        _ => "已连接 · 互联网状态待确认",
    }
}
fn label(s: &mut Scene, parent: &str, id: &str, rect: [f32; 4], value: &str, secondary: bool) {
    s.push(
        Widget::text(
            id,
            rect,
            value,
            if secondary {
                "text.secondary"
            } else {
                "text.primary"
            },
        )
        .parent(parent),
    );
}
fn button(
    s: &mut Scene,
    parent: &str,
    id: &str,
    rect: [f32; 4],
    value: &str,
    action: &str,
    enabled: bool,
    primary: bool,
) {
    s.push(
        Widget::button(
            id,
            rect,
            value,
            action,
            if primary {
                "button.primary"
            } else {
                "button.secondary"
            },
        )
        .parent(parent)
        .enabled(enabled),
    );
}
fn short(value: &str, max: usize) -> String {
    if value.chars().count() > max {
        format!("{}…", value.chars().take(max - 1).collect::<String>())
    } else {
        value.into()
    }
}
fn group(s: &mut Scene, id: &str, logical: [f32; 2], size: [f32; 2], offset: [f32; 3]) -> String {
    // Shell freezes this offset relative to the current horizon/eye height on creation,
    // and uses the same user-facing frame for rendering and ray hits.
    s.push(
        Widget::spatial_group(id, logical, size, offset).with_spatial_policy("head_horizon_spawn"),
    );
    let parent = format!("{id}-background");
    s.push(
        Widget::card(
            &parent,
            [0., 0., logical[0], logical[1]],
            "panel.background",
        )
        .parent(id),
    );
    parent
}
pub fn build(s: &mut Scene, state: &Value, wifi: &Wifi) {
    if wifi.editing
        && wifi.busy.is_none()
        && wifi
            .selected
            .as_ref()
            .is_some_and(AccessPoint::needs_password)
    {
        password_editor(s, wifi);
        keyboard(s, wifi);
        return;
    }
    let p = group(
        s,
        "wifi-panel",
        [640., 580.],
        [0.99, 0.90],
        [0., 1.63, -1.65],
    );
    let idle = wifi.busy.is_none();
    button(
        s,
        &p,
        "wifi-back",
        [24., 16., 112., 60.],
        "返回",
        "wifi:back",
        true,
        false,
    );
    label(
        s,
        &p,
        "wifi-title",
        [156., 24., 300., 42.],
        if wifi.selected.is_some() {
            "连接网络"
        } else {
            "Wi-Fi"
        },
        false,
    );
    if let Some(ap) = &wifi.selected {
        // A fresh authoritative sample controls connected/disconnected actions.
        let aps = access_points(state);
        let current = aps.iter().find(|n| {
            n.device == ap.device
                && n.ssid_bytes == ap.ssid_bytes
                && n.key_mgmt == ap.key_mgmt
                && n.connected
        });
        label(
            s,
            &p,
            "wifi-ssid",
            [28., 98., 584., 42.],
            &short(&ap.ssid, 25),
            false,
        );
        label(
            s,
            &p,
            "wifi-security",
            [28., 146., 584., 32.],
            &format!("{} · 信号 {}%", ap.security, ap.strength),
            true,
        );
        if let Some(current) = current {
            label(
                s,
                &p,
                "wifi-connected",
                [28., 206., 584., 36.],
                connected_label(current),
                false,
            );
            label(
                s,
                &p,
                "wifi-connected-hint",
                [28., 264., 584., 36.],
                "可在此断开当前无线网络连接",
                true,
            );
            button(
                s,
                &p,
                "wifi-disconnect",
                [28., 492., 584., 60.],
                "断开连接",
                "wifi:disconnect",
                idle,
                false,
            );
        } else if !ap.supported() {
            label(
                s,
                &p,
                "wifi-unsupported",
                [28., 224., 584., 36.],
                "此网络需要额外认证",
                false,
            );
            label(
                s,
                &p,
                "wifi-unsupported-hint",
                [28., 274., 584., 36.],
                "请先在系统网络设置中配置",
                true,
            );
        } else {
            let personal = ap.needs_password();
            if personal {
                label(
                    s,
                    &p,
                    "wifi-password-label",
                    [28., 202., 584., 32.],
                    "网络密码",
                    false,
                );
                let display = if wifi.password.is_empty() {
                    "点此输入密码".into()
                } else if wifi.revealed {
                    wifi.password
                        .chars()
                        .rev()
                        .take(24)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect()
                } else {
                    "•".repeat(wifi.password.chars().count().min(24))
                };
                button(
                    s,
                    &p,
                    "wifi-password",
                    [28., 246., 448., 64.],
                    &display,
                    "wifi:edit",
                    idle,
                    false,
                );
                button(
                    s,
                    &p,
                    "wifi-reveal",
                    [492., 246., 120., 64.],
                    if wifi.revealed { "隐藏" } else { "显示" },
                    "wifi:reveal",
                    idle,
                    false,
                );
                label(
                    s,
                    &p,
                    "wifi-password-hint",
                    [28., 324., 584., 32.],
                    if ap.saved.is_some() {
                        "已保存密码；留空可直接连接"
                    } else if ap.key_mgmt == "sae" {
                        "输入网络的 WPA3 密码"
                    } else {
                        "输入 8–63 字节密码或 64 位十六进制密钥"
                    },
                    true,
                );
                button(
                    s,
                    &p,
                    "wifi-keyboard",
                    [28., 374., 584., 60.],
                    if wifi.editing {
                        "收起空间键盘"
                    } else {
                        "打开空间键盘"
                    },
                    "wifi:edit",
                    idle,
                    false,
                );
            } else {
                label(
                    s,
                    &p,
                    "wifi-open-hint",
                    [28., 224., 584., 36.],
                    if ap.key_mgmt == "" {
                        "开放网络，无需密码"
                    } else if ap.key_mgmt == "owe" {
                        "此网络提供加密连接，无需密码"
                    } else {
                        "将使用系统已保存的认证配置"
                    },
                    false,
                );
                if ap.key_mgmt == "" {
                    label(
                        s,
                        &p,
                        "wifi-open-note",
                        [28., 276., 584., 36.],
                        "连接前请确认这是你信任的网络",
                        true,
                    );
                }
            }
            button(
                s,
                &p,
                "wifi-cancel",
                [28., 492., 280., 60.],
                "取消",
                "wifi:back",
                idle,
                false,
            );
            button(
                s,
                &p,
                "wifi-connect",
                [332., 492., 280., 60.],
                if wifi.busy == Some("connect") {
                    "正在连接…"
                } else {
                    "连接"
                },
                "wifi:connect",
                idle && valid_password(ap, &wifi.password),
                true,
            );
        }
        if !wifi.message.is_empty() {
            label(
                s,
                &p,
                "wifi-message",
                [28., 446., 584., 36.],
                &short(&wifi.message, 27),
                true,
            );
        }
    } else {
        let n = &state["network"]["state"];
        let available = state["network"]["available"] == true;
        let device = n["wifi_device_available"] == true;
        let enabled = n["wireless_enabled"] == true;
        let hardware = n["wireless_hardware_enabled"] == true;
        label(
            s,
            &p,
            "wifi-radio-label",
            [28., 98., 390., 36.],
            "无线网络",
            false,
        );
        button(
            s,
            &p,
            "wifi-radio",
            [464., 86., 148., 60.],
            if enabled { "已开启" } else { "已关闭" },
            "wifi:radio",
            idle && available && device && hardware,
            enabled,
        );
        label(
            s,
            &p,
            "wifi-list-title",
            [28., 148., 390., 32.],
            "附近的网络",
            true,
        );
        button(
            s,
            &p,
            "wifi-refresh",
            [464., 154., 148., 60.],
            if wifi.busy == Some("scan") {
                "扫描中…"
            } else {
                "刷新"
            },
            "wifi:scan",
            idle && enabled && device && hardware && n["wifi_usable"] == true,
            false,
        );
        let aps = access_points(state);
        let empty = if !available {
            Some("网络服务不可用，请稍后重试")
        } else if !device {
            Some("未检测到无线网卡")
        } else if !hardware {
            Some("无线硬件已禁用，请检查设备开关")
        } else if !enabled {
            Some("开启 Wi-Fi 后查看附近网络")
        } else if n["wifi_usable"] != true {
            Some("无线设备未就绪，请稍后重试")
        } else if n["scan_error"] == true {
            Some("无法读取无线列表，请刷新重试")
        } else if aps.is_empty() {
            Some("未发现网络，请刷新或靠近路由器")
        } else {
            None
        };
        let pages = aps.len().div_ceil(4).max(1);
        let page = wifi.page.min(pages - 1);
        if let Some(message) = empty {
            label(s, &p, "wifi-empty", [28., 278., 584., 66.], message, true);
        } else {
            for (i, ap) in aps.iter().skip(page * 4).take(4).enumerate() {
                let id = format!("wifi-ap-{}", ap.id);
                let y = 214. + i as f32 * 60.;
                // One large captured target contains both live text lines.
                button(
                    s,
                    &p,
                    &id,
                    [28., y, 584., 58.],
                    "",
                    &format!("wifi:select:{}", ap.id),
                    idle,
                    false,
                );
                if ap.connected {
                    s.push(
                        Widget::card(
                            &format!("{id}-selected"),
                            [0., 8., 4., 42.],
                            "button.primary",
                        )
                        .parent(&id),
                    );
                }
                s.push(
                    Widget::icon(
                        &format!("{id}-icon"),
                        [16., 11., 36., 36.],
                        "ukui-xr-network",
                        "text.primary",
                    )
                    .parent(&id),
                );
                label(
                    s,
                    &id,
                    &format!("{id}-name"),
                    [68., 2., 452., 28.],
                    &short(&ap.ssid, 27),
                    false,
                );
                let subtitle = if ap.connected {
                    connected_label(ap).into()
                } else {
                    format!(
                        "{} · {}%{}",
                        ap.security,
                        ap.strength,
                        if ap.saved.is_some() {
                            " · 已保存"
                        } else {
                            ""
                        }
                    )
                };
                label(
                    s,
                    &id,
                    &format!("{id}-caption"),
                    [68., 30., 488., 28.],
                    &subtitle,
                    true,
                );
            }
        }
        label(
            s,
            &p,
            "wifi-page",
            [28., 504., 265., 36.],
            &format!("{} 个网络 · {}/{}", aps.len(), page + 1, pages),
            true,
        );
        button(
            s,
            &p,
            "wifi-prev",
            [312., 496., 142., 60.],
            "上一页",
            "wifi:prev",
            idle && page > 0,
            false,
        );
        button(
            s,
            &p,
            "wifi-next",
            [470., 496., 142., 60.],
            "下一页",
            "wifi:next",
            idle && page + 1 < pages,
            false,
        );
        if !wifi.message.is_empty() {
            label(
                s,
                &p,
                "wifi-message",
                [28., 454., 584., 32.],
                &short(&wifi.message, 27),
                true,
            );
        }
    }
    if wifi.editing && wifi.selected.is_some() && idle {
        keyboard(s, wifi);
    }
}
fn password_editor(s: &mut Scene, wifi: &Wifi) {
    let ap = wifi.selected.as_ref().unwrap();
    // Three non-overlapping angular bands: compact form above the keyboard,
    // with the persistent taskbar below it. A new group freezes the new pose.
    let p = group(
        s,
        "wifi-password-panel",
        [640., 320.],
        [0.99, 0.496],
        [0., 1.83, -1.65],
    );
    button(
        s,
        &p,
        "wifi-back",
        [24., 12., 96., 52.],
        "返回",
        "wifi:back",
        true,
        false,
    );
    label(
        s,
        &p,
        "wifi-title",
        [140., 16., 300., 36.],
        "连接网络",
        false,
    );
    button(
        s,
        &p,
        "wifi-editor-done",
        [504., 12., 112., 52.],
        "完成",
        "wifi:done",
        true,
        false,
    );
    label(
        s,
        &p,
        "wifi-ssid",
        [28., 72., 584., 30.],
        &short(&ap.ssid, 26),
        false,
    );
    label(
        s,
        &p,
        "wifi-security",
        [28., 104., 584., 28.],
        &ap.security,
        true,
    );
    label(
        s,
        &p,
        "wifi-password-label",
        [28., 140., 584., 28.],
        "网络密码",
        false,
    );
    let display = if wifi.password.is_empty() {
        "点此输入密码".into()
    } else if wifi.revealed {
        wifi.password
            .chars()
            .rev()
            .take(24)
            .collect::<String>()
            .chars()
            .rev()
            .collect()
    } else {
        "•".repeat(wifi.password.chars().count().min(24))
    };
    button(
        s,
        &p,
        "wifi-password",
        [28., 180., 448., 60.],
        &display,
        "wifi:edit",
        true,
        false,
    );
    button(
        s,
        &p,
        "wifi-reveal",
        [492., 180., 120., 60.],
        if wifi.revealed { "隐藏" } else { "显示" },
        "wifi:reveal",
        true,
        false,
    );
    button(
        s,
        &p,
        "wifi-cancel",
        [28., 256., 280., 52.],
        "取消",
        "wifi:back",
        true,
        false,
    );
    button(
        s,
        &p,
        "wifi-connect",
        [332., 256., 280., 52.],
        "连接",
        "wifi:connect",
        valid_password(ap, &wifi.password),
        true,
    );
}
fn keyboard(s: &mut Scene, wifi: &Wifi) {
    let p = group(
        s,
        "wifi-keyboard-panel",
        [1000., 280.],
        [1.65, 0.462],
        [0., 1.295, -1.57],
    );
    label(
        s,
        &p,
        "wifi-keyboard-title",
        [26., 8., 700., 32.],
        "网络密码 · 仅输入到此连接",
        false,
    );
    let letters = ["1234567890", "qwertyuiop", "asdfghjkl", "zxcvbnm,."];
    let symbols = ["!@#$%^&*()", "[]{}<>-_+=", "\\|/?;:'\"", ",.`~"];
    for (row, keys) in (if wifi.symbols { symbols } else { letters })
        .iter()
        .enumerate()
    {
        let left = (1000. - keys.chars().count() as f32 * 90.) / 2.;
        for (col, ch) in keys.chars().enumerate() {
            let ch = if wifi.uppercase {
                ch.to_ascii_uppercase()
            } else {
                ch
            };
            s.push(
                Widget::button(
                    &format!("wifi-key-{row}-{col}"),
                    [left + col as f32 * 90., 48. + row as f32 * 42., 82., 38.],
                    &ch.to_string(),
                    &format!("wifi:char:{ch}"),
                    "keyboard.key",
                )
                .parent(&p),
            );
        }
    }
    for (id, x, width, value, action) in [
        (
            "shift",
            26.,
            142.,
            if wifi.uppercase { "小写" } else { "大写" },
            "wifi:shift",
        ),
        (
            "symbols",
            178.,
            136.,
            if wifi.symbols { "ABC" } else { "符号" },
            "wifi:symbols",
        ),
        ("space", 324., 260., "空格", "wifi:char: "),
        ("delete", 594., 142., "退格", "wifi:delete"),
        ("done", 746., 228., "完成", "wifi:done"),
    ] {
        s.push(
            Widget::button(
                &format!("wifi-key-{id}"),
                [x, 222., width, 38.],
                value,
                action,
                "keyboard.key",
            )
            .parent(&p),
        );
    }
}
