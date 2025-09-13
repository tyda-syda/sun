use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock};
use tokio::runtime::{Builder, Runtime};
use zbus::blocking::{connection::Connection, proxy::Proxy};
use zvariant::Value;

const BUS_NAME: &'static str = "org.freedesktop.Notifications";
const OBJ_PATH: &'static str = "/org/freedesktop/Notifications";
const IFACE: &'static str = "org.freedesktop.Notifications";

static ZBUS: LazyLock<Connection> = LazyLock::new(|| Connection::session().unwrap());
static RT: LazyLock<Runtime> = LazyLock::new(|| Builder::new_multi_thread().build().unwrap());

pub trait CloseHandler: FnMut() + Sync + Send + 'static {}

impl<T: FnMut() + Sync + Send + 'static> CloseHandler for T {}

#[derive(Hash, Copy, Clone, Eq, PartialEq, Debug)]
pub enum Timeout {
    Never,
    Millis(u32),
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum Urgency {
    Normal,
    Critical,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum Hint {
    Urgency(Urgency),
    Value(i32),
}

struct CloseHandlerContext {
    notif_id: Arc<AtomicU32>,
    close_handler: Arc<dyn CloseHandler>,
}

pub struct Notification {
    id: u32,
    pub summary: String,
    pub body: String,
    pub icon: String,
    pub timeout: i32,
    pub hints: HashMap<String, Hint>,
    close_handler_context: Option<CloseHandlerContext>,
}

impl Default for Timeout {
    fn default() -> Self {
        Timeout::Never
    }
}

impl From<i32> for Timeout {
    fn from(value: i32) -> Timeout {
        if value > 0 {
            return Timeout::Millis(value as u32);
        } else {
            return Timeout::Never;
        }
    }
}

impl From<Hint> for Value<'_> {
    fn from(value: Hint) -> Self {
        match value {
            Hint::Urgency(Urgency::Normal) => 1.into(),
            Hint::Urgency(Urgency::Critical) => 2.into(),
            Hint::Value(value) => value.into(),
        }
    }
}

impl std::default::Default for Notification {
    fn default() -> Self {
        Self {
            id: 0,
            summary: "".into(),
            body: "".into(),
            icon: "".into(),
            timeout: -1, // server decide
            hints: HashMap::new(),
            close_handler_context: None,
        }
    }
}

impl Notification {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn summary(&mut self, summary: &str) -> &mut Self {
        self.summary = summary.into();
        self
    }

    pub fn body(&mut self, body: &str) -> &mut Self {
        self.body = body.into();
        self
    }

    pub fn icon(&mut self, icon: &str) -> &mut Self {
        self.icon = icon.into();
        self
    }

    pub fn urgency(&mut self, urgency: Urgency) -> &mut Self {
        self.hint(Hint::Urgency(urgency));
        self
    }

    pub fn timeout(&mut self, timeout: Timeout) -> &mut Self {
        self.timeout = match timeout {
            Timeout::Millis(millis) => millis as i32,
            Timeout::Never => 0,
        };
        self
    }

    pub fn hint(&mut self, hint: Hint) -> &mut Self {
        match hint {
            Hint::Urgency(_) => self.hints.insert("urgency".into(), hint),
            Hint::Value(_) => self.hints.insert("value".into(), hint),
        };

        self
    }

    pub fn on_close(&mut self, handler: impl CloseHandler) -> &mut Self {
        let ctx = CloseHandlerContext {
            notif_id: Arc::new(AtomicU32::new(0)),
            close_handler: Arc::new(handler),
        };

        self.close_handler_context = Some(ctx);
        self
    }

    pub fn show(&mut self) {
        static ACTIONS: Vec<String> = Vec::new();

        let hints = self
            .hints
            .iter()
            .map(|(name, hint)| (name, (*hint).into()))
            .collect::<HashMap<_, Value<'_>>>();
        let notif_id = ZBUS
            .call_method(
                Some(BUS_NAME),
                OBJ_PATH,
                Some(IFACE),
                "Notify",
                &(
                    "sun",
                    self.id,
                    &self.icon,
                    &self.summary,
                    &self.body,
                    &ACTIONS,
                    hints,
                    self.timeout,
                ),
            )
            .unwrap()
            .body()
            .deserialize::<u32>()
            .unwrap();

        self.id = notif_id;

        if let Some(ref ctx) = self.close_handler_context {
            // start close handler only once
            if ctx.notif_id.load(Ordering::Relaxed) == 0 {
                let mut handler = Arc::clone(&ctx.close_handler);
                let notif_id = Arc::clone(&ctx.notif_id);
                let proxy = Proxy::new(&ZBUS, BUS_NAME, OBJ_PATH, IFACE).unwrap();

                RT.spawn(async move {
                    let handler = loop {
                        if let Some(handler) = Arc::get_mut(&mut handler) {
                            break handler;
                        }
                    };

                    loop {
                        for msg in proxy.receive_signal("NotificationClosed").unwrap() {
                            let body = msg.body();
                            let structure = body.deserialize::<zvariant::Structure>().unwrap();

                            if matches!(structure.fields()[0], Value::U32(id) if id == notif_id.load(Ordering::Relaxed)) {
                                handler();
                            }
                        }
                    }
                });
            }

            ctx.notif_id.store(notif_id, Ordering::Relaxed);
        }
    }
}
