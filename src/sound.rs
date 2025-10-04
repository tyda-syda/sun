use crate::config::{self, Config};
use crate::notif::{CloseReason, Hint, Notification, Timeout, Urgency};
use libpulse_binding as pa;
use pa::callbacks::ListResult;
use pa::context::introspect::{SinkInfo, SourceInfo};
use pa::context::subscribe::{Facility, InterestMaskSet};
use pa::context::{Context, FlagSet};
use pa::mainloop::standard::{IterateResult, Mainloop};
use pa::proplist::Proplist;
use pa::time::MicroSeconds;
use pa::volume::Volume;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};
use zbus::blocking::connection;
use zvariant;

// workaround for trait impl on external types error
macro_rules! pa_info_eq {
    ($info1:ident, $info2:ident) => {
        ($info1.index == $info2.index
            && $info1.volume.avg().0 == $info2.volume.avg().0
            && $info1.mute == $info2.mute)
    };
}

#[derive(Debug, Clone)]
struct PulseEvent {
    facility: Facility,
}

enum PollResult {
    Data(Vec<PulseEvent>),
    Timeout,
}

struct ContextHelper {
    main_loop: Mainloop,
    context: Context,
    event_queue: Rc<RefCell<Vec<PulseEvent>>>,
}

struct NotifHelper {
    zbus: zbus::blocking::Connection,
    sink_notif: Notification,
    source_notif: Notification,
}

impl ContextHelper {
    fn new() -> Self {
        let mut main_loop = Mainloop::new().unwrap();
        let mut context = Context::new(&main_loop, "dunst-centre").unwrap();

        context
            .connect(None, FlagSet::NOFAIL | FlagSet::NOAUTOSPAWN, None)
            .unwrap();

        loop {
            match main_loop.iterate(true) {
                IterateResult::Success(_) => {
                    if context.get_state() == pa::context::State::Ready {
                        context.subscribe(InterestMaskSet::SINK | InterestMaskSet::SOURCE, |res| {
                            if !res {
                                panic!("failed to subscribe on PulseAudio events")
                            }
                        });

                        break;
                    }
                }
                _ => panic!("cannot initialize PulseAudio context"),
            }
        }

        Self {
            main_loop,
            context,
            event_queue: Rc::new(RefCell::new(Vec::new())),
        }
    }

    fn subscribe(&mut self) {
        let event_queue = Rc::clone(&self.event_queue);

        self.context
            .set_subscribe_callback(Some(Box::new(
                move |facility, _operation, _index| match facility.unwrap() {
                    Facility::Sink | Facility::Source => {
                        let event = PulseEvent {
                            facility: facility.unwrap(),
                        };

                        event_queue.borrow_mut().push(event);
                    }
                    _ => (),
                },
            )));
    }

    fn poll_events(&mut self, timeout: Option<MicroSeconds>) -> PollResult {
        loop {
            let mut event_queue = self.event_queue.borrow_mut();

            if event_queue.len() > 0 {
                let event_queue_copy = event_queue.clone();

                event_queue.clear();

                return PollResult::Data(event_queue_copy);
            }

            drop(event_queue);

            self.main_loop.prepare(timeout).unwrap();

            let poll_ret = self.main_loop.poll().unwrap();
            let dispatched = self.main_loop.dispatch().unwrap();

            // EINTR is used to update configuration during blocking state
            fn interrupted() -> bool {
                unsafe {
                    *libc::__errno_location() == libc::EINTR
                }
            }

            if timeout.is_some() && poll_ret == 0 && dispatched == 0 {
                return PollResult::Timeout;
            } else if poll_ret == 0 && interrupted() {
                return PollResult::Data(Vec::new());
            }
        }
    }

    fn get_default_sink_info(&mut self) -> SinkInfo<'static> {
        let container = Rc::new(RefCell::new(None));
        let container_clone = Rc::clone(&container);

        self.context
            .introspect()
            .get_sink_info_by_name("@DEFAULT_SINK@", move |res| match res {
                ListResult::Item(info) => {
                    *container_clone.borrow_mut() = Some(info.to_owned());
                }
                ListResult::End => (),
                ListResult::Error => panic!("error iterate result"),
            });

        loop {
            match self.main_loop.iterate(true) {
                IterateResult::Success(_) => {
                    if container.borrow().is_some() {
                        return Rc::into_inner(container).unwrap().into_inner().unwrap();
                    }
                }
                _ => panic!("get default sink info error"),
            }
        }
    }

    fn get_default_source_info(&mut self) -> SourceInfo<'static> {
        let container = Rc::new(RefCell::new(None));
        let container_clone = Rc::clone(&container);

        self.context.introspect().get_source_info_by_name(
            "@DEFAULT_SOURCE@",
            move |res| match res {
                ListResult::Item(info) => {
                    *container_clone.borrow_mut() = Some(info.to_owned());
                }
                ListResult::End => (),
                ListResult::Error => panic!("error iterate result"),
            },
        );

        loop {
            match self.main_loop.iterate(true) {
                IterateResult::Success(_) => {
                    if container.borrow().is_some() {
                        return Rc::into_inner(container).unwrap().into_inner().unwrap();
                    }
                }
                _ => panic!("get default source info error"),
            }
        }
    }
}

impl From<config::Timeout> for Duration {
    fn from(val: config::Timeout) -> Self {
        match val {
            config::Timeout::Never => Duration::MAX,
            config::Timeout::Seconds(secs) => Duration::from_secs(secs),
            config::Timeout::Millis(millis) => Duration::from_millis(millis),
        }
    }
}

// PulseAudio doesn't have exact mapping to Timeout::Never, it uses Option::None
impl From<config::Timeout> for Option<MicroSeconds> {
    fn from(val: config::Timeout) -> Self {
        match val {
            config::Timeout::Never => None,
            config::Timeout::Seconds(secs) => MicroSeconds::from_secs(secs),
            config::Timeout::Millis(millis) => MicroSeconds::from_millis(millis),
        }
    }
}

impl From<config::Timeout> for Timeout {
    fn from(val: config::Timeout) -> Self {
        match val {
            config::Timeout::Never => Timeout::Millis(0),
            config::Timeout::Seconds(secs) => Timeout::Millis((secs * 1000) as u32),
            config::Timeout::Millis(millis) => Timeout::Millis(millis as u32),
        }
    }
}

impl NotifHelper {
    fn new() -> Self {
        Self {
            zbus: connection::Connection::system().unwrap(),
            sink_notif: Notification::new(),
            source_notif: Notification::new(),
        }
    }

    fn bluetooth_battery(&self, props: &Proplist) -> Option<u8> {
        let bluez_path = props.get_str("api.bluez5.path")?;
        let poll_timeout: Duration = Config::get()
            .sound
            .sink_bluetooth_battery_connect_poll_timeout
            .into();
        let start = SystemTime::now();
        let msg = loop {
            let msg = self.zbus.call_method(
                Some("org.bluez"),
                bluez_path.clone(),
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &("org.bluez.Battery1", "Percentage"),
            );

            if let Ok(_) = msg {
                break msg;
            }

            if start.elapsed().unwrap() >= poll_timeout {
                break msg;
            }
        };

        msg.ok()?
            .body()
            .deserialize::<zvariant::Structure>()
            .ok()?
            .fields()[0]
            .downcast_ref::<u8>()
            .ok()
    }

    fn show_sink_notification(
        &mut self,
        sink_info: &SinkInfo<'static>,
        only_low: bool,
    ) -> Option<MicroSeconds> {
        static NOTIF_CLOSED: AtomicBool = AtomicBool::new(false);
        static LOW_BATTERY: AtomicBool = AtomicBool::new(false);

        let mut poll_timeout = None;
        let config = Config::get();
        let config_sound = &config.sound;

        self.sink_notif
            .timeout(config_sound.sink_notification_timeout.into())
            .summary("Sound")
            .body("Volume")
            .icon(&config_sound.icon_path)
            .urgency(Urgency::Normal)
            .hint(Hint::Value(pa_volume_to_percent(sink_info.volume.avg().0)))
            .on_close(|reason| {
                if matches!(reason, CloseReason::ClosedByUser)
                    && LOW_BATTERY.load(Ordering::Relaxed)
                {
                    NOTIF_CLOSED.store(true, Ordering::Relaxed);
                }
            });

        if let Some(bus) = sink_info.proplist.get_str("device.bus") {
            if bus == "bluetooth" {
                self.sink_notif.body = sink_info.description.clone().unwrap().to_string();
            }
        }

        // we can receive new device event before it can register battery in dbus
        if let Some(battery) = self.bluetooth_battery(&sink_info.proplist) {
            poll_timeout = config_sound
                .sink_bluetooth_battery_poll_timeout
                .clone()
                .into();

            if battery <= config_sound.sink_bluetooth_low_battery_warn_at {
                LOW_BATTERY.store(true, Ordering::Relaxed);
                self.sink_notif.timeout(
                    config_sound
                        .sink_bluetooth_low_battery_notification_timeout
                        .into(),
                );
                self.sink_notif.urgency(Urgency::Critical);
                self.sink_notif
                    .body
                    .push_str(&format!(" ({battery}%) Low battery"));
            } else {
                LOW_BATTERY.store(false, Ordering::Relaxed);
                self.sink_notif.body.push_str(&format!(" ({}%)", battery));
            }
        }

        if sink_info.mute {
            self.sink_notif.summary.push_str(" muted");
            self.sink_notif.icon += &config_sound.sink_muted_icon;
        } else if poll_timeout.is_some() {
            self.sink_notif.icon += &config_sound.sink_bluetooth_icon;
        } else {
            self.sink_notif.icon += &config_sound.sink_icon;
        }

        if !only_low
            || (LOW_BATTERY.load(Ordering::Relaxed) && !NOTIF_CLOSED.load(Ordering::Relaxed))
        {
            self.sink_notif.show();
            NOTIF_CLOSED.store(false, Ordering::Relaxed);
        }

        poll_timeout
    }

    fn show_source_notification(&mut self, source_info: &SourceInfo<'static>) {
        let config_sound = Config::get().sound;

        self.source_notif
            .summary("Mic")
            .body("Volume")
            .urgency(Urgency::Normal)
            .timeout(config_sound.source_notification_timeout.into())
            .icon(&config_sound.icon_path)
            .hint(Hint::Value(pa_volume_to_percent(
                source_info.volume.avg().0,
            )));

        if source_info.mute {
            self.source_notif.summary.push_str(" muted");
            self.source_notif.icon += &config_sound.source_muted_icon;
        } else {
            self.source_notif.icon += &config_sound.source_icon;
        }

        self.source_notif.show();
    }
}

fn pa_volume_to_percent(volume: u32) -> i32 {
    ((volume * 100 + Volume::NORMAL.0 / 2) / Volume::NORMAL.0) as i32
}

pub fn routine() -> impl crate::Routine {
    || {
        let mut context_helper = ContextHelper::new();
        let mut notif_helper = NotifHelper::new();
        let mut default_sink = context_helper.get_default_sink_info();
        let mut default_source = context_helper.get_default_source_info();
        let mut poll_timeout = notif_helper
            .bluetooth_battery(&context_helper.get_default_sink_info().proplist)
            .map(|_| {
                Config::get()
                    .sound
                    .sink_bluetooth_battery_poll_timeout
                    .into()
            })
            .flatten();

        context_helper.subscribe();

        loop {
            if Config::get().sound.off {
                context_helper.main_loop.quit(pa::def::Retval(0));
                context_helper.context.disconnect();
                break;
            }

            match context_helper.poll_events(poll_timeout) {
                PollResult::Data(events) => {
                    for event in events {
                        match event.facility {
                            Facility::Sink => {
                                let current_default_sink = context_helper.get_default_sink_info();

                                if pa_info_eq!(current_default_sink, default_sink) {
                                    continue;
                                }

                                default_sink = current_default_sink;
                                poll_timeout =
                                    notif_helper.show_sink_notification(&default_sink, false);
                            }
                            Facility::Source => {
                                let current_default_source =
                                    context_helper.get_default_source_info();

                                if pa_info_eq!(current_default_source, default_source) {
                                    continue;
                                }

                                default_source = current_default_source;
                                notif_helper.show_source_notification(&default_source);
                            }
                            _ => (),
                        }
                    }
                }
                PollResult::Timeout => {
                    let sink_info = context_helper.get_default_sink_info();

                    poll_timeout = notif_helper.show_sink_notification(&sink_info, true);
                }
            }
        }
    }
}
