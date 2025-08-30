use crate::Message;
use inotify::{EventMask, Inotify, WatchMask};
use knuffel;
use knuffel::errors::Error as KnuffelError;
use std::io::ErrorKind;
use std::sync::mpsc::SyncSender;
use std::sync::RwLock;

const CONFIG_FILE: &'static str = "config.kdl";

const DEFAULT_ICON_PATH: &'static str = "/usr/share/icons/Adwaita/symbolic/";

const DEFAULT_SINK_ICON: &'static str = "status/audio-volume-high-symbolic.svg";
const DEFAULT_SINK_MUTED_ICON: &'static str = "status/audio-volume-muted-symbolic.svg";
const DEFAULT_SINK_BLUETOOTH_ICON: &'static str = "status/audio-volume-high-symbolic.svg";

const DEFAULT_SOURCE_ICON: &'static str = "status/microphone-sensetivity-high-symbolic.svg";
const DEFAULT_SOURCE_MUTED_ICON: &'static str = "status/microphone-sensetivity-muted-symbolic.svg";

const DEFAULT_KEYBOARD_ICON: &'static str = "devices/input-keyboard-symbolic.svg";

const DEFAULT_BRIGHTNESS_ICON: &'static str = "status/display-brightness-symbolic.svg";

const DEFAULT_BATTERY_FULL_ICON: &'static str = "status/battery-level-100-charged-symbolic.svg";
const DEFAULT_BATTERY_LOW_ICON: &'static str = "status/battery-caution-symbolic.svg";
const DEFAULT_BATTERY_CHARGING_ICON: &'static str =
    "status/battery-level-{level}-charging-symbolic.svg";
const DEFAULT_BATTERY_DISCHARGING_ICON: &'static str =
    "status/battery-level-{level}-symbolic.svg";

static CONFIG: RwLock<Option<Config>> = RwLock::new(None);

#[derive(knuffel::Decode, Clone, Debug)]
pub struct Config {
    #[knuffel(child, default)]
    pub sound: Sound,
    #[knuffel(child, default)]
    pub battery: Battery,
    #[knuffel(child, default)]
    pub keyboard: Keyboard,
    #[knuffel(child, default)]
    pub brightness: Brightness,
}

impl Config {
    pub fn get() -> Self {
        CONFIG
            .read()
            .unwrap()
            .clone()
            .expect("config must be initialized before accessing it")
    }

    pub fn update() -> Result<Self, KnuffelError> {
        let config = knuffel::parse::<Config>(
            CONFIG_FILE,
            &std::fs::read_to_string(CONFIG_FILE).unwrap_or(String::new()),
        )?;

        *CONFIG.write().unwrap() = Some(config.clone());

        Ok(config)
    }
}

#[derive(knuffel::Decode, Clone, Debug, Default)]
pub struct Battery {
    #[knuffel(child)]
    pub off: bool,
    #[knuffel(child, unwrap(argument), default = DEFAULT_ICON_PATH.into())]
    pub icon_path: String,
    #[knuffel(child, unwrap(argument), default = DEFAULT_BATTERY_FULL_ICON.into())]
    pub full_icon: String,
    #[knuffel(child, unwrap(argument), default = DEFAULT_BATTERY_LOW_ICON.into())]
    pub low_icon: String,
    #[knuffel(child, unwrap(argument), default = DEFAULT_BATTERY_CHARGING_ICON.into())]
    pub charging_icon: String,
    #[knuffel(child, unwrap(argument), default = true)]
    pub dynamic_charging_icon: bool,
    #[knuffel(child, unwrap(argument), default = DEFAULT_BATTERY_DISCHARGING_ICON.into())]
    pub discharging_icon: String,
    #[knuffel(child, unwrap(argument), default = true)]
    pub dynamic_discharging_icon: bool,
}

#[derive(knuffel::Decode, Clone, Debug, Default)]
pub struct Sound {
    #[knuffel(child)]
    pub off: bool,
    #[knuffel(child, unwrap(argument), default = DEFAULT_ICON_PATH.into())]
    pub icon_path: String,
    #[knuffel(child, unwrap(argument), default = DEFAULT_SINK_ICON.into())]
    pub sink_icon: String,
    #[knuffel(child, unwrap(argument), default = DEFAULT_SINK_MUTED_ICON.into())]
    pub sink_muted_icon: String,
    #[knuffel(child, unwrap(argument), default = DEFAULT_SINK_BLUETOOTH_ICON.into())]
    pub sink_bluetooth_icon: String,
    #[knuffel(child, unwrap(argument), default = 30)]
    pub sink_bluetooth_battery_poll_timeout: u64,
    #[knuffel(child, unwrap(argument), default = 15)]
    pub sink_bluetooth_low_battery_warn_at: u8,
    #[knuffel(child, unwrap(argument), default = -1)]
    pub sink_bluetooth_low_battery_timeout: i32,
    #[knuffel(child, unwrap(argument), default = 2500)]
    pub sink_notification_timeout: i32,
    #[knuffel(child, unwrap(argument), default = DEFAULT_SOURCE_ICON.into())]
    pub source_icon: String,
    #[knuffel(child, unwrap(argument), default = DEFAULT_SOURCE_MUTED_ICON.into())]
    pub source_muted_icon: String,
    #[knuffel(child, unwrap(argument), default = 2500)]
    pub source_notification_timeout: i32,
}

#[derive(knuffel::Decode, Clone, Debug, Default)]
pub struct Keyboard {
    #[knuffel(child)]
    pub off: bool,
    #[knuffel(child, unwrap(argument), default = DEFAULT_ICON_PATH.into())]
    pub icon_path: String,
    #[knuffel(child, unwrap(argument), default = DEFAULT_KEYBOARD_ICON.into())]
    pub icon: String,
}

#[derive(knuffel::Decode, Clone, Debug, Default)]
pub struct Brightness {
    #[knuffel(child)]
    pub off: bool,
    #[knuffel(child, unwrap(argument), default = DEFAULT_ICON_PATH.into())]
    pub icon_path: String,
    #[knuffel(child, unwrap(argument), default = DEFAULT_BRIGHTNESS_ICON.into())]
    pub icon: String,
}

pub fn routine(sender: SyncSender<Message>) -> impl crate::Routine {
    move || {
        let mut inotify = Inotify::init().unwrap();
        let mut buf =
            vec![0; inotify::get_buffer_size(&std::path::Path::new(CONFIG_FILE)).unwrap()];

        inotify
            .watches()
            .add(CONFIG_FILE, WatchMask::MODIFY)
            .unwrap();

        loop {
            for ev in inotify.read_events_blocking(&mut buf).unwrap() {
                match Config::update() {
                    Ok(config) => sender.send(Message::ConfigReload(config)).unwrap(),
                    Err(err) => sender.send(Message::ConfigReloadError(err)).unwrap(),
                }

                if ev.mask & EventMask::IGNORED == EventMask::IGNORED {
                    match inotify.watches().add(CONFIG_FILE, WatchMask::MODIFY) {
                        Err(err) if matches!(err.kind(), ErrorKind::NotFound) => (),
                        Err(err) => panic!("inotify add watch error:\n{err:#?}"),
                        _ => (),
                    }
                }
            }
        }
    }
}
