use notify_rust::{Notification, NotificationHandle};

pub struct NotifWrapper {
    inner: Notification,
    handle: Option<NotificationHandle>,
}

impl NotifWrapper {
    pub fn new() -> Self {
        Self {
            inner: Notification::new().finalize(),
            handle: None,
        }
    }

    pub fn show(&mut self) {
        if let Some(ref mut notif_handle) = self.handle {
            **notif_handle = self.inner.clone();
            notif_handle.update();
        } else {
            self.handle = Some(self.inner.show().unwrap());
        };
    }
}

impl std::ops::Deref for NotifWrapper {
    type Target = Notification;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl std::ops::DerefMut for NotifWrapper {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
