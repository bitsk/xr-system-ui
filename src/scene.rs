// SPDX-License-Identifier: GPL-3.0-or-later
use crate::notifications::Notice;
use serde_json::Value;
use xr_spatial_kit::{Scene, Widget, taskbar};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyboardPage {
    #[default]
    Letters,
    Symbols,
    Chinese,
}

#[derive(Clone, Debug)]
pub struct Panels {
    pub control: bool,
    pub keyboard: bool,
    pub notifications: bool,
    pub clipboard: bool,
    pub details: bool,
    pub wifi: bool,
    pub keyboard_page: KeyboardPage,
}
impl Default for Panels {
    fn default() -> Self {
        Self {
            control: true,
            keyboard: false,
            notifications: false,
            clipboard: false,
            details: false,
            wifi: false,
            keyboard_page: KeyboardPage::Letters,
        }
    }
}
impl Panels {
    pub fn command(&mut self, panel: &str, intent: &str) -> bool {
        if panel == "control" && intent == "toggle" && self.wifi {
            self.wifi = false;
            self.control = true;
            self.details = false;
            return true;
        }
        let value = match panel {
            "control" => &mut self.control,
            "keyboard" => &mut self.keyboard,
            "notifications" => &mut self.notifications,
            "clipboard" => &mut self.clipboard,
            "details" => &mut self.details,
            "wifi" => &mut self.wifi,
            _ => return false,
        };
        *value = match intent {
            "show" => true,
            "hide" => false,
            "toggle" => !*value,
            _ => return false,
        };
        if panel == "control" && self.control {
            self.details = false;
            self.wifi = false;
        }
        true
    }
}
fn available(state: &Value, key: &str) -> bool {
    state[key]["available"].as_bool() == Some(true)
}
fn field<'a>(state: &'a Value, key: &str, name: &str) -> &'a str {
    state[key]["state"][name].as_str().unwrap_or("")
}
fn short(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}
fn text(s: &mut Scene, parent: &str, id: &str, rect: [f32; 4], value: &str, secondary: bool) {
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
    label: &str,
    action: &str,
    enabled: bool,
) {
    let icon = match id {
        "keyboard-toggle" | "ime-us" | "keyboard-ime-us" => "input-keyboard",
        "notifications-toggle" => "preferences-system-notifications",
        "clipboard-toggle" => "edit-paste",
        "details-toggle" => "document-properties",
        "applications" => "applications-system",
        "refresh" => "view-refresh",
        "ime-pinyin" | "keyboard-ime-pinyin" | "chinese" => "accessories-character-map",
        _ => "",
    };
    s.push(
        Widget::button(id, rect, label, action, "button.secondary")
            .with_icon(icon)
            .parent(parent)
            .enabled(enabled),
    );
}
fn spatial(
    s: &mut Scene,
    id: &str,
    logical: [f32; 2],
    size: [f32; 2],
    position: [f32; 3],
    rotation: [f32; 3],
) -> String {
    let mut group = Widget::spatial_group(id, logical, size, position).with_rotation(rotation);
    group.style_id = "panel.background".into();
    s.push(group);
    let background = format!("{id}-background");
    s.push(
        Widget::card(
            &background,
            [0., 0., logical[0], logical[1]],
            "panel.background",
        )
        .parent(id),
    );
    background
}
fn network_label(state: &Value) -> String {
    if !available(state, "network") {
        return "网络状态待连接".into();
    }
    let connection_state = field(state, "network", "state");
    let connected = connection_state.starts_with("connected");
    let devices = state["network"]["state"]["devices"].as_array();
    let kind = devices
        .and_then(|d| {
            d.iter().find(|d| {
                d["state"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("connected"))
                    && d["kind"] != "loopback"
            })
        })
        .and_then(|d| d["kind"].as_str())
        .unwrap_or("");
    format!(
        "{} · {}",
        match kind {
            "ethernet" => "以太网",
            "wifi" => "无线网络",
            _ => "网络",
        },
        if connected {
            "已连接"
        } else if connection_state == "connecting" {
            "正在连接"
        } else {
            "未连接"
        }
    )
}

pub fn build(
    state: &Value,
    notices: &[Notice],
    active_workspace: Option<u64>,
    clipboard: Option<&str>,
    last_result: &str,
    panels: &Panels,
    wifi: &crate::wifi::Wifi,
) -> Scene {
    let mut s = Scene::new(
        [600., 700.],
        [0.60, 0.70],
        [1.18, 1.6, -1.74],
        "world_locked",
    )
    .with_rotation([0., -18., 0.]);
    status_island(&mut s, state);
    if panels.control && !panels.wifi {
        s.push(Widget::card(
            "panel",
            [0., 0., 600., 700.],
            "panel.background",
        ));
        if panels.details {
            details(&mut s, state, active_workspace, last_result);
        } else {
            control(&mut s, state);
        }
    }
    if panels.wifi {
        crate::wifi::build(&mut s, state, wifi);
    }
    if panels.keyboard && !panels.wifi {
        keyboard(&mut s, state, panels.keyboard_page);
    }
    if panels.notifications {
        notifications(&mut s, state, notices);
    }
    if panels.clipboard {
        clipboard_panel(&mut s, clipboard);
    }
    s
}
fn status_island(s: &mut Scene, state: &Value) {
    let island = "status-island";
    let mut group = Widget::spatial_group(
        island,
        taskbar::SIDE_LOGICAL,
        taskbar::SIDE_SIZE,
        taskbar::RIGHT_POSITION,
    )
    .with_spatial_policy(taskbar::POLICY)
    .with_rotation(taskbar::ROTATION);
    group.style_id = "dock.background".into();
    s.push(group);
    let parent = "status-island-background";
    s.push(
        Widget::card(
            parent,
            [0., 0., taskbar::SIDE_LOGICAL[0], taskbar::LOGICAL_HEIGHT],
            "dock.background",
        )
        .parent(island),
    );
    let devices = state["network"]["state"]["devices"].as_array();
    let connected_kind = devices
        .and_then(|devices| {
            devices.iter().find(|d| {
                d["state"]
                    .as_str()
                    .is_some_and(|v| v.starts_with("connected"))
                    && d["kind"] != "loopback"
            })
        })
        .and_then(|d| d["kind"].as_str());
    let network_icon = match connected_kind {
        Some("wifi") => "ukui-xr-network",
        Some("ethernet") => "ukui-xr-wired",
        Some(_) => "ukui-xr-network",
        None => "ukui-xr-network-offline",
    };
    s.push(
        Widget::button(
            "status-network",
            [26., 10., 52., 52.],
            "",
            "wifi:open",
            "dock.status-icon",
        )
        .with_icon(network_icon)
        .parent(parent),
    );
    for (id, x) in [
        ("status-network-separator", 96.),
        ("status-volume-separator", 184.),
        ("status-settings-separator", 350.),
    ] {
        s.push(Widget::card(id, [x, 20., 1., 32.], "taskbar.separator").parent(parent));
    }
    let mute = state["volume"]["state"]["muted"].as_bool();
    s.push(
        Widget::button(
            "status-volume",
            [112., 10., 52., 52.],
            "",
            "mute-toggle",
            "dock.status-icon",
        )
        .with_icon(if mute == Some(true) {
            "ukui-xr-audio-muted"
        } else {
            "ukui-xr-audio"
        })
        .enabled(available(state, "volume") && mute.is_some())
        .parent(parent),
    );
    let percent = state["battery"]["state"]["percent"]
        .as_f64()
        .filter(|n| available(state, "battery") && n.is_finite() && *n >= 0. && *n <= 100.);
    let icon = percent
        .map(|p| format!("ukui-xr-battery-{p:.0}"))
        .unwrap_or_else(|| "ukui-xr-battery-unknown".into());
    s.push(
        Widget::icon(
            "status-battery",
            [206., 20., 40., 32.],
            &icon,
            "dock.status-icon",
        )
        .parent(parent),
    );
    let label = percent
        .map(|p| format!("{p:.0}%"))
        .unwrap_or_else(|| "--".into());
    s.push(
        Widget::text(
            "status-battery-percent",
            [262., 12., 76., 48.],
            &label,
            "taskbar.status",
        )
        .parent(parent),
    );
    s.push(
        Widget::button(
            "system",
            [366., 10., 52., 52.],
            "",
            "ui:control",
            "dock.status-icon",
        )
        .with_icon("ukui-xr-status-settings")
        .parent(parent),
    );
    if state["notifications"]["count"].as_u64().unwrap_or(0) > 0 {
        s.push(
            Widget::card(
                "status-notification-dot",
                [56., 14., 6., 6.],
                "taskbar.running",
            )
            .parent(parent),
        );
    }
}
fn control(s: &mut Scene, state: &Value) {
    text(
        s,
        "panel",
        "title",
        [28., 22., 430., 38.],
        "控制中心",
        false,
    );
    button(
        s,
        "panel",
        "control-close",
        [516., 20., 56., 44.],
        "×",
        "ui:control",
        true,
    );
    text(
        s,
        "panel",
        "theme-title",
        [28., 88., 400., 30.],
        "应用外观",
        false,
    );
    let theme = field(state, "theme", "name");
    s.push(
        Widget::button(
            "theme-light",
            [28., 130., 264., 54.],
            "浅色",
            "theme:ukui-light",
            if theme == "ukui-light" {
                "button.primary"
            } else {
                "button.secondary"
            },
        )
        .with_icon("weather-clear")
        .parent("panel")
        .enabled(available(state, "theme")),
    );
    s.push(
        Widget::button(
            "theme-dark",
            [308., 130., 264., 54.],
            "深色",
            "theme:ukui-dark",
            if theme == "ukui-dark" {
                "button.primary"
            } else {
                "button.secondary"
            },
        )
        .with_icon("weather-clear-night")
        .parent("panel")
        .enabled(available(state, "theme")),
    );
    text(
        s,
        "panel",
        "audio-title",
        [28., 220., 300., 30.],
        "声音",
        false,
    );
    let percent = state["volume"]["state"]["percent"].as_f64().unwrap_or(0.);
    let muted = state["volume"]["state"]["muted"].as_bool().unwrap_or(false);
    let volume = if available(state, "volume") {
        format!("{percent:.0}%")
    } else {
        "未连接输出".into()
    };
    text(
        s,
        "panel",
        "volume-status",
        [376., 220., 196., 30.],
        &volume,
        true,
    );
    s.push(
        Widget::slider(
            "volume",
            [28., 272., 376., 44.],
            percent as f32,
            0.,
            100.,
            "volume",
            "slider.primary",
        )
        .parent("panel")
        .enabled(available(state, "volume")),
    );
    s.push(
        Widget::button(
            "mute",
            [428., 266., 144., 56.],
            if muted { "取消静音" } else { "静音" },
            "mute-toggle",
            "button.secondary",
        )
        .with_icon(if muted {
            "audio-volume-muted"
        } else {
            "audio-volume-high"
        })
        .parent("panel")
        .enabled(available(state, "volume")),
    );
    text(
        s,
        "panel",
        "network-title",
        [28., 362., 400., 30.],
        "网络",
        false,
    );
    s.push(
        Widget::button(
            "network-card",
            [28., 406., 544., 62.],
            "",
            "wifi:open",
            "button.secondary",
        )
        .parent("panel"),
    );
    text(
        s,
        "network-card",
        "network-status",
        [20., 14., 440., 34.],
        &network_label(state),
        false,
    );
    let mode = state["taskbar"]["mode"].as_str().unwrap_or("fixed");
    for (id, x, label, selected) in [
        ("fixed", 28., "固定任务栏", "fixed"),
        ("follow", 308., "跟随头部", "follow"),
    ] {
        s.push(
            Widget::button(
                &format!("taskbar-{id}"),
                [x, 482., 264., 48.],
                label,
                &format!("taskbar:{selected}"),
                if mode == selected {
                    "button.primary"
                } else {
                    "button.secondary"
                },
            )
            .parent("panel"),
        );
    }
    text(
        s,
        "panel",
        "tools-title",
        [28., 546., 400., 30.],
        "快捷工具",
        false,
    );
    for (id, x, label, action) in [
        ("keyboard-toggle", 28., "键盘", "ui:keyboard"),
        ("notifications-toggle", 214., "通知", "ui:notifications"),
        ("clipboard-toggle", 400., "剪贴板", "ui:clipboard"),
    ] {
        button(s, "panel", id, [x, 588., 172., 48.], label, action, true);
    }
    button(
        s,
        "panel",
        "details-toggle",
        [28., 650., 264., 36.],
        "系统详情",
        "ui:details",
        true,
    );
    button(
        s,
        "panel",
        "applications",
        [308., 650., 264., 36.],
        "应用库",
        "launcher-toggle",
        true,
    );
}
fn details(s: &mut Scene, state: &Value, active_workspace: Option<u64>, last_result: &str) {
    text(
        s,
        "panel",
        "details-title",
        [28., 22., 420., 38.],
        "系统详情",
        false,
    );
    button(
        s,
        "panel",
        "details-back",
        [452., 20., 120., 44.],
        "返回",
        "ui:details",
        true,
    );
    text(
        s,
        "panel",
        "workspace-title",
        [28., 90., 544., 30.],
        "工作区",
        false,
    );
    for n in 0..4 {
        s.push(
            Widget::button(
                &format!("workspace-{n}"),
                [28. + n as f32 * 140., 132., 124., 46.],
                &(n + 1).to_string(),
                &format!("workspace:{n}"),
                if active_workspace == Some(n) {
                    "button.primary"
                } else {
                    "button.secondary"
                },
            )
            .parent("panel"),
        );
    }
    let current = field(state, "ime", "current");
    text(
        s,
        "panel",
        "ime-status",
        [28., 214., 544., 30.],
        &format!(
            "输入法 · {}",
            if current.is_empty() {
                "未启用"
            } else {
                current
            }
        ),
        true,
    );
    button(
        s,
        "panel",
        "ime-pinyin",
        [28., 254., 264., 46.],
        "拼音",
        "ime:pinyin",
        available(state, "ime") && state["ime"]["state"]["pinyin_available"] == true,
    );
    button(
        s,
        "panel",
        "ime-us",
        [308., 254., 264., 46.],
        "英文",
        "ime:keyboard-us",
        available(state, "ime"),
    );
    text(
        s,
        "panel",
        "network-detail",
        [28., 334., 544., 30.],
        &network_label(state),
        true,
    );
    text(
        s,
        "panel",
        "wifi-detail",
        [28., 372., 544., 28.],
        if state["network"]["state"]["wifi_device_available"] == true {
            "检测到无线设备"
        } else {
            "当前没有无线设备"
        },
        true,
    );
    let audio = if available(state, "volume") {
        format!(
            "音频 · {}{}",
            short(field(state, "volume", "sink_name"), 25),
            if state["volume"]["state"]["virtual_sink"] == true {
                "（虚拟输出）"
            } else {
                ""
            }
        )
    } else {
        "音频端点未连接".into()
    };
    text(
        s,
        "panel",
        "audio-detail",
        [28., 416., 544., 30.],
        &audio,
        true,
    );
    button(
        s,
        "panel",
        "refresh",
        [28., 470., 264., 46.],
        "刷新状态",
        "refresh",
        true,
    );
    button(
        s,
        "panel",
        "chinese",
        [308., 470., 264., 46.],
        "提交中文示例",
        "chinese-commit",
        true,
    );
    text(
        s,
        "panel",
        "result-title",
        [28., 554., 544., 28.],
        "最近操作",
        true,
    );
    for (i, line) in last_result
        .chars()
        .collect::<Vec<_>>()
        .chunks(25)
        .take(2)
        .enumerate()
    {
        text(
            s,
            "panel",
            &format!("result-{i}"),
            [28., 594. + i as f32 * 34., 544., 30.],
            &line.iter().collect::<String>(),
            true,
        );
    }
}
fn keyboard(s: &mut Scene, state: &Value, page: KeyboardPage) {
    let p = spatial(
        s,
        "keyboard-panel",
        [1000., 280.],
        [1.053, 0.29484],
        [0., 0.54, -1.27],
        [16.5, 0., 0.],
    );
    text(
        s,
        &p,
        "keyboard-title",
        [22., 10., 240., 30.],
        "空间键盘",
        false,
    );
    for (id, x, label, action, selected) in [
        (
            "keyboard-letters",
            300.,
            "ABC",
            "keyboard:letters",
            KeyboardPage::Letters,
        ),
        (
            "keyboard-symbols",
            414.,
            "符号",
            "keyboard:symbols",
            KeyboardPage::Symbols,
        ),
        (
            "keyboard-chinese",
            528.,
            "中文标点",
            "keyboard:chinese",
            KeyboardPage::Chinese,
        ),
    ] {
        s.push(
            Widget::button(
                id,
                [x, 8., 104., 32.],
                label,
                action,
                if page == selected {
                    "button.primary"
                } else {
                    "button.secondary"
                },
            )
            .parent(&p),
        );
    }
    button(
        s,
        &p,
        "keyboard-ime-pinyin",
        [654., 8., 112., 32.],
        "拼音",
        "ime:pinyin",
        available(state, "ime") && state["ime"]["state"]["pinyin_available"] == true,
    );
    button(
        s,
        &p,
        "keyboard-ime-us",
        [776., 8., 112., 32.],
        "EN",
        "ime:keyboard-us",
        available(state, "ime"),
    );
    button(
        s,
        &p,
        "keyboard-close",
        [930., 8., 48., 32.],
        "×",
        "ui:keyboard",
        true,
    );
    if page == KeyboardPage::Letters {
        for (row, letters) in ["1234567890", "qwertyuiop", "asdfghjkl", "zxcvbnm"]
            .iter()
            .enumerate()
        {
            let left = (1000. - letters.len() as f32 * 90.) / 2.;
            for (col, ch) in letters.chars().enumerate() {
                s.push(
                    Widget::button(
                        &format!("key-{ch}"),
                        [left + col as f32 * 90., 48. + row as f32 * 42., 82., 36.],
                        &ch.to_ascii_uppercase().to_string(),
                        &format!("key:{}", [2, 16, 30, 44][row] + col),
                        "keyboard.key",
                    )
                    .parent(&p),
                );
            }
        }
        for (id, x, label, action) in [
            ("key-comma", 50., ",", "key:51"),
            ("key-period", 860., ".", "key:52"),
        ] {
            s.push(
                Widget::button(id, [x, 174., 82., 36.], label, action, "keyboard.key").parent(&p),
            );
        }
    } else {
        // US punctuation uses evdev (including Shift) so keyboard-only clients
        // and the active IME receive real keys. Chinese punctuation is literal.
        let symbols = [
            ("!", "key:42+2"),
            ("@", "key:42+3"),
            ("#", "key:42+4"),
            ("$", "key:42+5"),
            ("%", "key:42+6"),
            ("^", "key:42+7"),
            ("&", "key:42+8"),
            ("*", "key:42+9"),
            ("(", "key:42+10"),
            (")", "key:42+11"),
            ("[", "key:26"),
            ("]", "key:27"),
            ("{", "key:42+26"),
            ("}", "key:42+27"),
            ("<", "key:42+51"),
            (">", "key:42+52"),
            ("-", "key:12"),
            ("_", "key:42+12"),
            ("=", "key:13"),
            ("+", "key:42+13"),
            ("\\", "key:43"),
            ("|", "key:42+43"),
            ("/", "key:53"),
            ("?", "key:42+53"),
            (";", "key:39"),
            (":", "key:42+39"),
            ("'", "key:40"),
            ("\"", "key:42+40"),
            (",", "key:51"),
            (".", "key:52"),
            ("`", "key:41"),
            ("~", "key:42+41"),
        ];
        let chinese = "，。、；：？！…—·“”‘’（）【】《》〈〉「」『』〔〕［］｛｝";
        let keys: Vec<(String, String)> = if page == KeyboardPage::Symbols {
            symbols
                .iter()
                .map(|(label, action)| (label.to_string(), action.to_string()))
                .collect()
        } else {
            chinese
                .chars()
                .map(|ch| (ch.to_string(), format!("type:{ch}")))
                .collect()
        };
        for (i, (label, action)) in keys.iter().enumerate() {
            s.push(
                Widget::button(
                    &format!("key-symbol-{i}"),
                    [
                        140. + (i % 8) as f32 * 90.,
                        48. + (i / 8) as f32 * 42.,
                        82.,
                        36.,
                    ],
                    label,
                    action,
                    "keyboard.key",
                )
                .parent(&p),
            );
        }
    }
    for (id, x, w, label, action) in [
        ("keyboard-copy", 50., 136., "复制", "shortcut:copy"),
        ("keyboard-space", 194., 442., "空格", "key:57"),
        ("keyboard-backspace", 644., 142., "退格", "key:14"),
        ("keyboard-enter", 794., 156., "回车", "key:28"),
    ] {
        s.push(Widget::button(id, [x, 222., w, 40.], label, action, "keyboard.key").parent(&p));
    }
}
fn notifications(s: &mut Scene, state: &Value, notices: &[Notice]) {
    let p = spatial(
        s,
        "notifications-panel",
        [600., 340.],
        [0.60, 0.34],
        [0., 2.38, -1.78],
        [0., 0., 0.],
    );
    text(
        s,
        &p,
        "notification-title",
        [24., 20., 450., 36.],
        "通知",
        false,
    );
    button(
        s,
        &p,
        "notifications-close",
        [520., 18., 56., 40.],
        "×",
        "ui:notifications",
        true,
    );
    if let Some(notice) = notices.last() {
        text(
            s,
            &p,
            "notice-summary",
            [24., 86., 552., 32.],
            &short(&notice.summary, 25),
            false,
        );
        for (i, line) in notice
            .body
            .chars()
            .collect::<Vec<_>>()
            .chunks(26)
            .take(3)
            .enumerate()
        {
            text(
                s,
                &p,
                &format!("notice-body-{i}"),
                [24., 132. + i as f32 * 34., 552., 30.],
                &line.iter().collect::<String>(),
                true,
            );
        }
        button(
            s,
            &p,
            &format!("notice-close-{}", notice.id),
            [24., 274., 264., 44.],
            "关闭通知",
            &format!("notice-close:{}", notice.id),
            true,
        );
        if let Some(label) = notice.actions.get(1) {
            button(
                s,
                &p,
                &format!("notice-action-{}", notice.id),
                [308., 274., 264., 44.],
                &short(label, 12),
                &format!("notice-action:{}:0", notice.id),
                true,
            );
        }
    } else {
        text(
            s,
            &p,
            "notice-empty",
            [24., 118., 552., 36.],
            if state["notifications"]["owned_in_isolated_bus"] == true {
                "暂无通知"
            } else {
                "通知服务未开启"
            },
            true,
        );
    }
}
fn clipboard_panel(s: &mut Scene, clipboard: Option<&str>) {
    let p = spatial(
        s,
        "clipboard-panel",
        [600., 300.],
        [0.60, 0.30],
        [-1.18, 1.6, -1.74],
        [0., 18., 0.],
    );
    text(
        s,
        &p,
        "clipboard-title",
        [24., 20., 450., 36.],
        "剪贴板",
        false,
    );
    button(
        s,
        &p,
        "clipboard-close",
        [520., 18., 56., 40.],
        "×",
        "ui:clipboard",
        true,
    );
    if let Some(value) = clipboard {
        for (i, line) in value
            .chars()
            .collect::<Vec<_>>()
            .chunks(26)
            .take(3)
            .enumerate()
        {
            text(
                s,
                &p,
                &format!("clipboard-text-{i}"),
                [24., 90. + i as f32 * 34., 552., 30.],
                &line.iter().collect::<String>(),
                true,
            );
        }
    } else {
        text(
            s,
            &p,
            "clipboard-empty",
            [24., 100., 552., 32.],
            "剪贴板为空",
            true,
        );
    }
    button(
        s,
        &p,
        "clipboard-copy",
        [24., 234., 264., 44.],
        "复制选中内容",
        "shortcut:copy",
        true,
    );
    button(
        s,
        &p,
        "clipboard-paste",
        [308., 234., 264., 44.],
        "粘贴到窗口",
        "shortcut:paste",
        clipboard.is_some(),
    );
}
