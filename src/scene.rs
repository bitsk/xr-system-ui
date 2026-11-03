// SPDX-License-Identifier: GPL-3.0-or-later
use crate::notifications::Notice;
use serde_json::Value;
use xr_spatial_kit::{Scene, Widget};

fn available(state: &Value, key: &str) -> bool {
    state[key]["available"].as_bool() == Some(true)
}
fn label(state: &Value, key: &str, field: &str) -> String {
    if !available(state, key) {
        return "不可用".into();
    }
    let v = &state[key]["state"][field];
    v.as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| v.to_string())
}
fn short(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

pub fn build(
    state: &Value,
    notices: &[Notice],
    active_workspace: Option<u64>,
    clipboard: Option<&str>,
    last_result: &str,
) -> Scene {
    let mut s = Scene::new(
        [720.0, 1120.0],
        [0.72, 1.12],
        [0.95, 1.45, -1.65],
        "world_locked",
    );
    s.push(Widget::card(
        "panel",
        [0.0, 0.0, 720.0, 1120.0],
        "panel.background",
    ));
    s.push(Widget::text(
        "title",
        [20.0, 16.0, 680.0, 36.0],
        "UKUI XR · 空间控制中心",
        "text.primary",
    ));
    s.push(Widget::text(
        "theme-status",
        [20.0, 62.0, 680.0, 32.0],
        &format!(
            "主题 {} · 字体 {}",
            label(state, "theme", "name"),
            label(state, "theme", "font")
        ),
        "text.secondary",
    ));
    s.push(
        Widget::button(
            "theme-light",
            [20.0, 104.0, 200.0, 40.0],
            "浅色主题",
            "theme:ukui-light",
            "button.secondary",
        )
        .enabled(available(state, "theme")),
    );
    s.push(
        Widget::button(
            "theme-dark",
            [232.0, 104.0, 200.0, 40.0],
            "深色主题",
            "theme:ukui-dark",
            "button.secondary",
        )
        .enabled(available(state, "theme")),
    );
    let percent = state["volume"]["state"]["percent"].as_f64().unwrap_or(0.0);
    let virtual_suffix = if state["volume"]["state"]["virtual_sink"] == true {
        " · 虚拟输出"
    } else {
        ""
    };
    let volume_text = if available(state, "volume") {
        format!("音量 {:.0}%{}", percent, virtual_suffix)
    } else {
        "音频端点不可用".into()
    };
    s.push(Widget::text(
        "volume-status",
        [20.0, 158.0, 680.0, 30.0],
        &volume_text,
        "text.primary",
    ));
    s.push(
        Widget::slider(
            "volume",
            [20.0, 202.0, 430.0, 30.0],
            percent as f32,
            0.0,
            100.0,
            "volume",
            "slider.primary",
        )
        .enabled(available(state, "volume")),
    );
    let muted = state["volume"]["state"]["muted"].as_bool().unwrap_or(false);
    s.push(
        Widget::button(
            "mute",
            [474.0, 194.0, 210.0, 40.0],
            if muted { "取消静音" } else { "静音" },
            "mute-toggle",
            "button.secondary",
        )
        .enabled(available(state, "volume")),
    );
    let wifi = if state["network"]["state"]["wifi_device_available"] == true {
        "Wi-Fi可用"
    } else {
        "无Wi-Fi设备"
    };
    s.push(Widget::text(
        "network-status",
        [20.0, 248.0, 680.0, 32.0],
        &format!(
            "网络 {} / {} · {}",
            label(state, "network", "state"),
            label(state, "network", "connectivity"),
            wifi
        ),
        "text.primary",
    ));
    s.push(Widget::button(
        "refresh",
        [20.0, 290.0, 180.0, 40.0],
        "刷新真实状态",
        "refresh",
        "button.secondary",
    ));
    s.push(Widget::text(
        "workspace-title",
        [20.0, 342.0, 680.0, 30.0],
        &format!(
            "空间工作区 · 当前 {}",
            active_workspace
                .map(|x| (x + 1).to_string())
                .unwrap_or_else(|| "未确认".into())
        ),
        "text.primary",
    ));
    for n in 0..4 {
        s.push(Widget::button(
            &format!("workspace-{n}"),
            [20.0 + n as f32 * 172.0, 382.0, 160.0, 40.0],
            &format!("工作区 {}", n + 1),
            &format!("workspace:{n}"),
            if active_workspace == Some(n) {
                "button.primary"
            } else {
                "button.secondary"
            },
        ));
    }
    s.push(Widget::text(
        "clipboard-status",
        [20.0, 438.0, 680.0, 30.0],
        &format!(
            "隔离剪贴板：{}",
            clipboard
                .map(|s| short(s, 30))
                .unwrap_or_else(|| "等待Wayland selection".into())
        ),
        "text.primary",
    ));
    s.push(Widget::button(
        "clipboard-copy",
        [20.0, 478.0, 250.0, 40.0],
        "复制 XR 测试文本",
        "clipboard-copy",
        "button.secondary",
    ));
    s.push(
        Widget::button(
            "clipboard-paste",
            [284.0, 478.0, 250.0, 40.0],
            "粘贴到当前窗口",
            "clipboard-paste",
            "button.secondary",
        )
        .enabled(clipboard.is_some()),
    );
    s.push(Widget::text(
        "ime-status",
        [20.0, 534.0, 680.0, 30.0],
        &format!(
            "输入法 {} · 中文候选由真实IME提供",
            label(state, "ime", "current")
        ),
        "text.primary",
    ));
    s.push(
        Widget::button(
            "ime-pinyin",
            [20.0, 574.0, 200.0, 40.0],
            "拼音输入法",
            "ime:pinyin",
            "button.secondary",
        )
        .enabled(available(state, "ime") && state["ime"]["state"]["pinyin_available"] == true),
    );
    s.push(
        Widget::button(
            "ime-us",
            [232.0, 574.0, 200.0, 40.0],
            "英文键盘",
            "ime:keyboard-us",
            "button.secondary",
        )
        .enabled(available(state, "ime")),
    );
    let notification_title = if state["notifications"]["owned_in_isolated_bus"] == true {
        format!(
            "通知 · {} 条（独立 D-Bus）",
            state["notifications"]["count"].as_u64().unwrap_or(0)
        )
    } else {
        "通知 · 未启动独立通知服务".into()
    };
    s.push(Widget::text(
        "notification-title",
        [20.0, 628.0, 680.0, 30.0],
        &notification_title,
        "text.primary",
    ));
    if let Some(notice) = notices.last() {
        s.push(Widget::text(
            "notice-summary",
            [20.0, 666.0, 570.0, 30.0],
            &short(&notice.summary, 48),
            "text.primary",
        ));
        s.push(Widget::text(
            "notice-body",
            [20.0, 704.0, 680.0, 30.0],
            &short(&notice.body, 60),
            "text.secondary",
        ));
        s.push(Widget::button(
            "notice-close",
            [576.0, 662.0, 110.0, 36.0],
            "关闭",
            &format!("notice-close:{}", notice.id),
            "button.secondary",
        ));
        if let Some(action_label) = notice.actions.get(1) {
            s.push(Widget::button(
                "notice-action",
                [20.0, 744.0, 240.0, 36.0],
                &short(action_label, 20),
                &format!("notice-action:{}:0", notice.id),
                "button.primary",
            ));
        }
    }
    s.push(Widget::text(
        "result",
        [20.0, 790.0, 680.0, 32.0],
        &short(last_result, 60),
        "text.secondary",
    ));
    s.push(Widget::text(
        "keyboard-title",
        [20.0, 830.0, 680.0, 30.0],
        "空间键盘 · 输入发往已确认焦点窗口",
        "text.primary",
    ));
    for (row, text) in ["1234567890", "qwertyuiop", "asdfghjkl", "zxcvbnm"]
        .iter()
        .enumerate()
    {
        let margin = (720.0 - text.len() as f32 * 68.0) / 2.0;
        for (col, ch) in text.chars().enumerate() {
            s.push(Widget::button(
                &format!("key-{ch}"),
                [
                    margin + col as f32 * 68.0,
                    870.0 + row as f32 * 42.0,
                    62.0,
                    36.0,
                ],
                &ch.to_string(),
                &format!("key:{}", [2, 16, 30, 44][row] + col),
                "button.secondary",
            ));
        }
    }
    for (id, x, w, text, action) in [
        ("space", 20.0, 180.0, "空格", "key:57"),
        ("backspace", 212.0, 130.0, "退格", "key:14"),
        ("enter", 354.0, 130.0, "回车", "key:28"),
        ("chinese", 496.0, 204.0, "提交中文示例", "chinese-commit"),
    ] {
        s.push(Widget::button(
            id,
            [x, 1044.0, w, 40.0],
            text,
            action,
            "button.secondary",
        ));
    }
    // Paint the background before every control independent of sibling ID order.
    for widget in &mut s.widgets {
        if widget.id != "panel" {
            widget.parent_id = "panel".into();
        }
    }
    s
}
