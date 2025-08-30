use inotify::{EventMask, Inotify, WatchMask};
use knuffel;
use std::sync::mpsc::SyncSender;
use std::sync::RwLock;

const CONFIG_FILE: &'static str = "config.kdl";
static CONFIG: RwLock<Option<Config>> = RwLock::new(None);

#[derive(knuffel::Decode, Clone, Debug)]
pub struct Config {
    #[knuffel(child, unwrap(argument))]
    pub icon_path: String,
    #[knuffel(child, default = true)]
    pub panic_notification: bool,
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

    pub fn update() {
        let config = std::fs::read_to_string(CONFIG_FILE).unwrap();

        match knuffel::parse(CONFIG_FILE, &config) {
            Ok(parsed) => *CONFIG.write().unwrap() = Some(parsed),
            Err(err) => println!("error while parsing {CONFIG_FILE}:\n{err:#?}"),
        }
    }
}

#[derive(knuffel::Decode, Clone, Debug)]
pub struct Battery {
    #[knuffel(child)]
    pub off: bool,
}

impl std::default::Default for Battery {
    fn default() -> Self {
        Self { off: false }
    }
}

#[derive(knuffel::Decode, Clone, Debug, Default)]
pub struct Sound {
    #[knuffel(child)]
    pub off: bool,
    #[knuffel(child, unwrap(argument))]
    pub sink_format: Option<String>,
    #[knuffel(child, unwrap(argument))]
    pub source_format: Option<String>,
    #[knuffel(child, unwrap(argument))]
    pub icon_path: Option<String>,
    #[knuffel(child, unwrap(argument))]
    pub icon_sink: String,
    #[knuffel(child, unwrap(argument))]
    pub icon_sink_muted: String,
    #[knuffel(child, unwrap(argument))]
    pub icon_sink_bluetooth: String,
    #[knuffel(child, unwrap(argument))]
    pub icon_source: String,
    #[knuffel(child, unwrap(argument), default = 30)]
    pub sink_bluetooth_battery_poll_timeout: u64,
    #[knuffel(child, unwrap(argument), default = 15)]
    pub sink_bluetooth_low_battery_warn_at: u8,
    #[knuffel(child, unwrap(argument), default = -1)]
    pub sink_bluetooth_low_battery_timeout: i32,
}

#[derive(knuffel::Decode, Clone, Debug)]
pub struct Keyboard {
    #[knuffel(child)]
    pub off: bool,
}

impl std::default::Default for Keyboard {
    fn default() -> Self {
        Self { off: false }
    }
}

#[derive(knuffel::Decode, Clone, Debug)]
pub struct Brightness {
    #[knuffel(child)]
    pub off: bool,
}

impl std::default::Default for Brightness {
    fn default() -> Self {
        Self { off: false }
    }
}

pub fn routine(sender: SyncSender<crate::Message>) -> impl crate::Routine {
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
                Config::update();

                sender.send(crate::Message::ConfigReload).unwrap();

                if ev.mask & EventMask::IGNORED == EventMask::IGNORED {
                    inotify
                        .watches()
                        .add(CONFIG_FILE, WatchMask::MODIFY)
                        .unwrap();
                }
            }
        }
    }
}
