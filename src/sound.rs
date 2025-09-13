use crate::config::Config;
use crate::notif::{Hint, Notification, Timeout, Urgency};
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

            if timeout.is_some() && poll_ret == 0 && dispatched == 0 {
                return PollResult::Timeout;
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
        let body = self
            .zbus
            .call_method(
                Some("org.bluez"),
                bluez_path,
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &("org.bluez.Battery1", "Percentage"),
            )
            .ok()?
            .body();

        body.deserialize::<zvariant::Structure>().ok()?.fields()[0]
            .downcast_ref::<u8>()
            .ok()
    }

    fn show_sink_notification(
        &mut self,
        sink_info: &SinkInfo<'static>,
        only_low: bool,
    ) -> Option<MicroSeconds> {
        static NOTIF_CLOSED: AtomicBool = AtomicBool::new(false);

        let mut poll_timeout = None;
        let mut low_battery = false;
        let config = Config::get();
        let config_sound = &config.sound;

        self.sink_notif
            .timeout(Timeout::from(config_sound.sink_notification_timeout))
            .summary("Sound")
            .body("Volume")
            .icon(&config_sound.icon_path)
            .hint(Hint::Value(pa_volume_to_percent(sink_info.volume.avg().0)))
            .on_close(|_| NOTIF_CLOSED.store(true, Ordering::Relaxed));

        if let Some(bus) = sink_info.proplist.get_str("device.bus") {
            if bus == "bluetooth" {
                self.sink_notif.body = sink_info.description.clone().unwrap().to_string();
            }
        }

        // we can receive new device event before it can register battery in dbus
        if let Some(battery) = self.bluetooth_battery(&sink_info.proplist) {
            poll_timeout = Some(
                MicroSeconds::from_secs(config_sound.sink_bluetooth_battery_poll_timeout).unwrap(),
            );

            if battery <= config_sound.sink_bluetooth_low_battery_warn_at {
                let timeout = config_sound.sink_bluetooth_low_battery_timeout;
                low_battery = true;

                self.sink_notif.timeout(Timeout::from(timeout));
                self.sink_notif.urgency(Urgency::Critical);
                self.sink_notif
                    .body
                    .push_str(&format!(" ({}%) Low battery", battery));
            } else {
                let _ = NOTIF_CLOSED.compare_exchange(
                    true,
                    false,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
                self.sink_notif.body.push_str(&format!(" ({}%)", battery));
            }
        };

        if sink_info.mute {
            self.sink_notif.summary.push_str(" muted");
            self.sink_notif.icon += &config_sound.sink_muted_icon;
        } else if poll_timeout.is_some() {
            self.sink_notif.icon += &config_sound.sink_bluetooth_icon;
        } else {
            self.sink_notif.icon += &config_sound.sink_icon;
        }

        if !only_low || (low_battery && !NOTIF_CLOSED.load(Ordering::Relaxed)) {
            self.sink_notif.show();
        }

        poll_timeout
    }

    fn show_source_notification(&mut self, source_info: &SourceInfo<'static>) {
        let config_sound = Config::get().sound;

        self.source_notif
            .summary("Mic")
            .body("Volume")
            .urgency(Urgency::Normal)
            .timeout(Timeout::from(config_sound.source_notification_timeout))
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
                MicroSeconds::from_millis(Config::get().sound.sink_bluetooth_battery_poll_timeout)
                    .unwrap()
            });

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
