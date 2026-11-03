// SPDX-License-Identifier: GPL-3.0-or-later
use anyhow::{Result, ensure};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};
use zbus::{
    Connection,
    fdo::{DBusProxy, RequestNameFlags, RequestNameReply},
    object_server::SignalEmitter,
    zvariant::OwnedValue,
};

pub const PATH: &str = "/org/freedesktop/Notifications";
#[derive(Clone, Debug, Serialize)]
pub struct Notice {
    pub id: u32,
    pub app: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<String>,
    #[serde(skip)]
    pub deadline: Option<Instant>,
}
#[derive(Default)]
pub struct NoticeStore {
    pub entries: Mutex<BTreeMap<u32, Notice>>,
    next: AtomicU32,
}
impl NoticeStore {
    pub fn snapshot(&self) -> Vec<Notice> {
        self.entries.lock().unwrap().values().cloned().collect()
    }
}
struct Notifications {
    store: Arc<NoticeStore>,
}
#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    fn get_capabilities(&self) -> Vec<&str> {
        vec!["body", "actions"]
    }
    fn get_server_information(&self) -> (&str, &str, &str, &str) {
        ("UKUI XR", "openKylin XR", "0.1.0", "1.2")
    }
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        _app_icon: &str,
        summary: &str,
        body: &str,
        actions: Vec<String>,
        _hints: BTreeMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> zbus::fdo::Result<u32> {
        if app_name.len() > 256
            || summary.len() > 512
            || body.len() > 4096
            || actions.len() > 32
            || actions.len() % 2 != 0
            || actions.iter().any(|s| s.len() > 256)
        {
            return Err(zbus::fdo::Error::InvalidArgs(
                "notification exceeds supported limits".into(),
            ));
        }
        let mut entries = self.store.entries.lock().unwrap();
        let id = if replaces_id != 0 && entries.contains_key(&replaces_id) {
            replaces_id
        } else {
            if entries.len() >= 64 {
                return Err(zbus::fdo::Error::LimitsExceeded(
                    "notification limit reached".into(),
                ));
            }
            let n = self
                .store
                .next
                .fetch_add(1, Ordering::Relaxed)
                .checked_add(1)
                .ok_or_else(|| {
                    zbus::fdo::Error::LimitsExceeded("notification identifiers exhausted".into())
                })?;
            n
        };
        let timeout = if expire_timeout < 0 {
            5000
        } else {
            expire_timeout as u64
        };
        let deadline = if timeout == 0 {
            None
        } else {
            Some(Instant::now() + Duration::from_millis(timeout))
        };
        entries.insert(
            id,
            Notice {
                id,
                app: app_name.into(),
                summary: summary.into(),
                body: body.into(),
                actions,
                deadline,
            },
        );
        println!("XR_NOTIFICATION_RECEIVED={id}");
        Ok(id)
    }
    async fn close_notification(
        &self,
        id: u32,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        let removed = self.store.entries.lock().unwrap().remove(&id).is_some();
        if removed {
            Self::notification_closed(&emitter, id, 3).await?;
        }
        Ok(())
    }
    #[zbus(signal)]
    async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn action_invoked(
        emitter: &SignalEmitter<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;
}

pub struct NotificationServer {
    pub connection: Connection,
    pub store: Arc<NoticeStore>,
}
impl NotificationServer {
    pub async fn isolated() -> Result<Self> {
        ensure!(
            std::env::var("XR_SYSTEM_UI_ISOLATED_BUS").as_deref() == Ok("1"),
            "notification ownership requires explicit XR_SYSTEM_UI_ISOLATED_BUS=1"
        );
        let connection = Connection::session().await?;
        let bus = DBusProxy::new(&connection).await?;
        for name in [
            "org.gnome.SessionManager",
            "org.ukui.SettingsDaemon",
            "org.freedesktop.Notifications",
        ] {
            ensure!(
                !bus.name_has_owner(name.try_into()?).await?,
                "refusing notification ownership on a desktop/already-owned bus: {name}"
            );
        }
        let store = Arc::new(NoticeStore::default());
        connection
            .object_server()
            .at(
                PATH,
                Notifications {
                    store: store.clone(),
                },
            )
            .await?;
        let reply = connection
            .request_name_with_flags(
                "org.freedesktop.Notifications",
                RequestNameFlags::DoNotQueue.into(),
            )
            .await?;
        ensure!(
            matches!(
                reply,
                RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner
            ),
            "notification name not acquired"
        );
        println!("XR_NOTIFICATIONS_OWNED=1 BUS_SCOPE=explicit-isolated");
        Ok(Self { connection, store })
    }
    pub async fn expire(&self) -> Result<()> {
        let expired: Vec<u32> = self
            .store
            .snapshot()
            .into_iter()
            .filter(|n| n.deadline.is_some_and(|t| t <= Instant::now()))
            .map(|n| n.id)
            .collect();
        for id in expired {
            self.dismiss(id, 1).await?;
        }
        Ok(())
    }
    pub async fn dismiss(&self, id: u32, reason: u32) -> Result<()> {
        let removed = self.store.entries.lock().unwrap().remove(&id).is_some();
        if removed {
            Notifications::notification_closed(
                &SignalEmitter::new(&self.connection, PATH)?,
                id,
                reason,
            )
            .await?;
        }
        Ok(())
    }
    pub async fn invoke(&self, id: u32, index: usize) -> Result<()> {
        let notice = self
            .store
            .entries
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("notification disappeared"))?;
        let key = notice
            .actions
            .get(index * 2)
            .ok_or_else(|| anyhow::anyhow!("notification action absent"))?;
        Notifications::action_invoked(&SignalEmitter::new(&self.connection, PATH)?, id, key)
            .await?;
        self.dismiss(id, 2).await
    }
}
