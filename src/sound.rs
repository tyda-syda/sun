use libpulse_binding as pa;
use notify_rust::{Hint, Timeout, Urgency};
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
use zbus::blocking::connection;
use zvariant;
use crate::notif::NotifWrapper;

const DEFAULT_NOTIFICATION_TIMEOUT: i32 = 2500; // millis
const BLUETOOTH_POLL_TIMEOUT: u64 = 30; // secs
const BLUETOOTH_BATTERY_WARN_AT: u8 = 15;

#[derive(Clone, Debug)]
struct PulseEventData {
    index: u32,
    description: String,
    volume: u32,
    mute: bool,
    props: Proplist,
}

#[derive(Clone)]
struct PulseEvent {
    data: PulseEventData,
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
    notif: NotifWrapper,
}

impl PartialEq<SinkInfo<'static>> for PulseEventData {
    fn eq(&self, other: &SinkInfo<'static>) -> bool {
        self.volume == other.volume.avg().0 && self.mute == other.mute
    }
}

impl PartialEq<SourceInfo<'static>> for PulseEventData {
    fn eq(&self, other: &SourceInfo<'static>) -> bool {
        self.volume == other.volume.avg().0 && self.mute == other.mute
    }
}

impl From<SinkInfo<'static>> for PulseEventData {
    fn from(value: SinkInfo<'static>) -> Self {
        Self {
            index: value.index,
            description: value.description.unwrap().to_string(),
            volume: value.volume.avg().0,
            mute: value.mute,
            props: value.proplist,
        }
    }
}

impl From<SourceInfo<'static>> for PulseEventData {
    fn from(value: SourceInfo<'static>) -> Self {
        Self {
            index: value.index,
            description: value.description.unwrap().to_string(),
            volume: value.volume.avg().0,
            mute: value.mute,
            props: value.proplist,
        }
    }
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
        let intro = self.context.introspect();
        let event_queue = Rc::clone(&self.event_queue);

        self.context
            .set_subscribe_callback(Some(Box::new(move |facility, _operation, _index| {
                let event_queue = event_queue.clone(); // necessary due to inner move closure

                match facility.unwrap() {
                    Facility::Sink => {
                        intro.get_sink_info_by_name("@DEFAULT_SINK@", move |res| match res {
                            ListResult::Item(info) => {
                                let event = PulseEvent {
                                    data: PulseEventData::from(info.to_owned()),
                                    facility: facility.unwrap(),
                                };

                                event_queue.borrow_mut().push(event);
                            }
                            ListResult::End => (),
                            ListResult::Error => panic!("error iterate result"),
                        });
                    }
                    Facility::Source => {
                        intro.get_source_info_by_name("@DEFAULT_SOURCE@", move |res| match res {
                            ListResult::Item(info) => {
                                let event = PulseEvent {
                                    data: PulseEventData::from(info.to_owned()),
                                    facility: facility.unwrap(),
                                };

                                event_queue.borrow_mut().push(event);
                            }
                            ListResult::End => (),
                            ListResult::Error => panic!("error iterate result"),
                        });
                    }
                    _ => (),
                }
            })));
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
            notif: NotifWrapper::new(),
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
        sink_info: &PulseEventData,
        only_low: bool,
    ) -> Option<MicroSeconds> {
        let mut poll_timeout = None;
        let mut low_battery = false;

        self.notif
            .urgency(Urgency::Normal)
            .timeout(DEFAULT_NOTIFICATION_TIMEOUT)
            .summary("Sound")
            .body("Volume")
            .hint(Hint::CustomInt(
                "value".into(),
                pa_volume_to_percent(sink_info.volume),
            ))
            .icon("/usr/share/icons/Adwaita/symbolic/status/audio-volume-high-symbolic.svg");

        if let Some(bus) = sink_info.props.get_str("device.bus") {
            if bus == "bluetooth" {
                self.notif.body = sink_info.description.clone();
            }
        }

        // we can receive new device event before it can register its battery in dbus
        if let Some(battery) = self.bluetooth_battery(&sink_info.props) {
            poll_timeout = Some(MicroSeconds::from_secs(BLUETOOTH_POLL_TIMEOUT).unwrap());

            if battery <= BLUETOOTH_BATTERY_WARN_AT {
                low_battery = true;

                self.notif.timeout = Timeout::Never;
                self.notif.urgency(Urgency::Critical);
                self.notif
                    .body
                    .push_str(&format!(" ({}%) Low battery", battery));
            } else {
                self.notif.body.push_str(&format!(" ({}%)", battery));
            }
        };

        if sink_info.mute {
            self.notif.summary.push_str(" muted");
            self.notif.icon = String::from(
                "/usr/share/icons/Adwaita/symbolic/status/audio-volume-muted-symbolic.svg",
            );
        }

        if !only_low || low_battery {
            self.notif.show();
        }

        poll_timeout
    }

    fn show_source_notification(&mut self, source_info: &PulseEventData) {
        self.notif
            .summary("Mic")
            .body("Volume")
            .urgency(Urgency::Normal)
            .timeout(DEFAULT_NOTIFICATION_TIMEOUT)
            .icon(
                "/usr/share/icons/Adwaita/symbolic/status/microphone-sensitivity-high-symbolic.svg",
            )
            .hint(Hint::CustomInt(
                "value".into(),
                pa_volume_to_percent(source_info.volume),
            ));

        if source_info.mute {
            self.notif.summary.push_str(" muted");
            self.notif.icon = String::from(
                "/usr/share/icons/Adwaita/symbolic/status/microphone-disabled-symbolic.svg",
            );
        }

        self.notif.show();
    }
}

fn pa_volume_to_percent(volume: u32) -> i32 {
    ((volume * 100 + Volume::NORMAL.0 / 2) / Volume::NORMAL.0) as i32
}

pub fn routine() -> impl crate::Routine {
    || {
        let mut context_helper = ContextHelper::new();
        let mut notif_helper = NotifHelper::new();
        let mut poll_timeout = notif_helper
            .bluetooth_battery(&context_helper.get_default_sink_info().proplist)
            .map(|_| MicroSeconds::from_secs(BLUETOOTH_POLL_TIMEOUT).unwrap());

        context_helper.subscribe();

        loop {
            let mut default_sink = context_helper.get_default_sink_info();
            let mut default_source = context_helper.get_default_source_info();

            match context_helper.poll_events(poll_timeout) {
                PollResult::Data(events) => {
                    for event in events {
                        let info = event.data;

                        match event.facility {
                            Facility::Sink => {
                                let current_default_sink = context_helper.get_default_sink_info();

                                if default_sink.index != current_default_sink.index {
                                    default_sink = current_default_sink;
                                } else if info.index != default_sink.index || info == default_sink {
                                    continue;
                                }

                                poll_timeout = notif_helper.show_sink_notification(&info, false);
                            }
                            Facility::Source => {
                                let current_default_source =
                                    context_helper.get_default_source_info();

                                if default_source.index != current_default_source.index {
                                    default_source = current_default_source;
                                    continue;
                                } else if info.index != default_source.index
                                    || info == default_source
                                {
                                    continue;
                                }

                                notif_helper.show_source_notification(&info);
                            }
                            _ => continue,
                        }
                    }
                }
                PollResult::Timeout => {
                    let sink_info = PulseEventData::from(context_helper.get_default_sink_info());

                    poll_timeout = notif_helper.show_sink_notification(&sink_info, true);
                }
            }
        }
    }
}
