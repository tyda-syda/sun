use crate::Message;
use inotify::{EventMask, Inotify, WatchMask};
use knuffel;
use knuffel::errors::Error as KnuffelError;
use regex::Regex;
use std::env::var;
use std::fs::{File, create_dir_all, exists};
use std::io::{Write, ErrorKind};
use std::str::FromStr;
use std::sync::mpsc::Sender;
use std::sync::RwLock;

const CONFIG_FILE: &'static str = "config.kdl";

const DEFAULT_ICON_PATH: &'static str = "/usr/share/icons/Adwaita/symbolic/";
const DEFAULT_ERROR_ICON: &'static str =
    "/usr/share/icons/Adwaita/symbolic/status/computer-fail-symbolic.svg";

const DEFAULT_SINK_ICON: &'static str = "status/audio-volume-high-symbolic.svg";
const DEFAULT_SINK_MUTED_ICON: &'static str = "status/audio-volume-muted-symbolic.svg";
const DEFAULT_SINK_BLUETOOTH_ICON: &'static str = "status/audio-volume-high-symbolic.svg";

const DEFAULT_SOURCE_ICON: &'static str = "status/microphone-sensitivity-high-symbolic.svg";
const DEFAULT_SOURCE_MUTED_ICON: &'static str = "status/microphone-sensitivity-muted-symbolic.svg";

const DEFAULT_KEYBOARD_ICON: &'static str = "devices/input-keyboard-symbolic.svg";

const DEFAULT_BRIGHTNESS_ICON: &'static str = "status/display-brightness-symbolic.svg";

const DEFAULT_BATTERY_TARGET: &'static str = "BAT0";
const DEFAULT_BATTERY_FULL_ICON: &'static str = "status/battery-level-100-charged-symbolic.svg";
const DEFAULT_BATTERY_LOW_ICON: &'static str = "status/battery-caution-symbolic.svg";
const DEFAULT_BATTERY_CHARGING_ICON: &'static str =
    "status/battery-level-{level}-charging-symbolic.svg";
const DEFAULT_BATTERY_DISCHARGING_ICON: &'static str = "status/battery-level-{level}-symbolic.svg";

static CONFIG: RwLock<Option<Config>> = RwLock::new(None);

#[derive(Default, Clone, Copy, Debug)]
pub enum Timeout {
    #[default]
    Never,
    Seconds(u64),
    Millis(u64),
}

impl<S: knuffel::traits::ErrorSpan> knuffel::traits::DecodeScalar<S> for Timeout {
    fn type_check(
        type_name: &Option<knuffel::span::Spanned<knuffel::ast::TypeName, S>>,
        ctx: &mut knuffel::decode::Context<S>,
    ) {
        if let Some(name) = type_name {
            ctx.emit_error(knuffel::errors::DecodeError::unexpected(
                name,
                "sound.sink-bluetooth-battery-poll-timeout",
                "unexpected type name",
            ));
        }
    }

    fn raw_decode(
        val: &knuffel::span::Spanned<knuffel::ast::Literal, S>,
        _: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, knuffel::errors::DecodeError<S>> {
        let reg_non_num = Regex::new(r"\D").unwrap();
        let reg_secs = Regex::new(r"^Secs\(\d+\)$").unwrap();
        let reg_millis = Regex::new(r"^Millis\(\d+\)$").unwrap();

        match &**val {
            knuffel::ast::Literal::String(lit) => {
                if reg_secs.is_match(lit) {
                    Ok(Timeout::Seconds(
                        u64::from_str(&reg_non_num.replace_all(lit, "")).unwrap(),
                    ))
                } else if reg_millis.is_match(lit) {
                    Ok(Timeout::Millis(
                        u64::from_str(&reg_non_num.replace_all(lit, "")).unwrap(),
                    ))
                } else if "Never" == &**lit {
                    Ok(Timeout::Never)
                } else {
                    Err(knuffel::errors::DecodeError::unexpected(
                        val,
                        "sound.sink-bluetooth-battery-poll-timeout",
                        format!("unknown Timeout value: {}", lit.to_owned()),
                    ))
                }
            }
            other => Err(knuffel::errors::DecodeError::unexpected(
                val,
                "sound.sink-bluetooth-battery-poll-timeout",
                format!("unexpected token: {other:?}"),
            )),
        }
    }
}

#[derive(knuffel::Decode, Clone, Debug)]
pub struct Config {
    #[knuffel(child, unwrap(argument), default = DEFAULT_ERROR_ICON.into())]
    pub error_icon: String,
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
            &std::fs::read_to_string(CONFIG_FILE).unwrap_or(include_str!("../config.kdl").into()),
        )?;

        *CONFIG.write().unwrap() = Some(config.clone());

        Ok(config)
    }
}

#[derive(knuffel::Decode, Clone, Debug, Default)]
pub struct Battery {
    #[knuffel(child)]
    pub off: bool,
    #[knuffel(child, unwrap(argument), default = DEFAULT_BATTERY_TARGET.into())]
    pub target: String,
    #[knuffel(child, unwrap(argument), default = Timeout::Millis(15 * 1000))]
    pub poll_timeout: Timeout,
    #[knuffel(child, unwrap(argument), default = 15)]
    pub warn_at: u8,
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
    #[knuffel(child, unwrap(argument), default = Timeout::Seconds(30))]
    pub sink_bluetooth_battery_poll_timeout: Timeout,
    #[knuffel(child, unwrap(argument), default = Timeout::Millis(1000))]
    pub sink_bluetooth_battery_connect_poll_timeout: Timeout,
    #[knuffel(child, unwrap(argument), default = 15)]
    pub sink_bluetooth_low_battery_warn_at: u8,
    #[knuffel(child, unwrap(argument), default = Timeout::Never)]
    pub sink_bluetooth_low_battery_notification_timeout: Timeout,
    #[knuffel(child, unwrap(argument), default = Timeout::Millis(2500))]
    pub sink_notification_timeout: Timeout,
    #[knuffel(child, unwrap(argument), default = DEFAULT_SOURCE_ICON.into())]
    pub source_icon: String,
    #[knuffel(child, unwrap(argument), default = DEFAULT_SOURCE_MUTED_ICON.into())]
    pub source_muted_icon: String,
    #[knuffel(child, unwrap(argument), default = Timeout::Millis(2500))]
    pub source_notification_timeout: Timeout,
}

#[derive(knuffel::Decode, Clone, Debug, Default)]
pub struct Keyboard {
    #[knuffel(child)]
    pub off: bool,
    #[knuffel(child, unwrap(argument), default = DEFAULT_ICON_PATH.into())]
    pub icon_path: String,
    #[knuffel(child, unwrap(argument), default = DEFAULT_KEYBOARD_ICON.into())]
    pub icon: String,
    #[knuffel(child, unwrap(argument), default = Timeout::Seconds(1))]
    pub notification_timeout: Timeout,
}

#[derive(knuffel::Decode, Clone, Debug, Default)]
pub struct Brightness {
    #[knuffel(child)]
    pub off: bool,
    #[knuffel(child, unwrap(argument), default = DEFAULT_ICON_PATH.into())]
    pub icon_path: String,
    #[knuffel(child, unwrap(argument), default = DEFAULT_BRIGHTNESS_ICON.into())]
    pub icon: String,
    #[knuffel(child, unwrap(argument))]
    pub target: Option<String>,
    #[knuffel(child, unwrap(argument), default = Timeout::Seconds(1))]
    pub notification_timeout: Timeout,
}

fn ensure_cfg_file() -> String {
    let cfg_dir = var("XDG_CONFIG_HOME")
        .map(|dir| dir + "/sun")
        .unwrap_or_else(|_| format!("{}/.config/sun", var("HOME").unwrap()));
    let cfg_file = format!("{cfg_dir}/{CONFIG_FILE}");

    let _ = create_dir_all(&cfg_dir);

    if let Ok(false) | Err(_) = exists(&cfg_file) {
        let mut file = File::options()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(&cfg_file)
            .unwrap();

        file.write_all(include_bytes!("../config.kdl")).unwrap();
    };

    cfg_file
}

pub fn routine(sender: Sender<Message>) -> impl crate::Routine {
    move || {
        let mut inotify = Inotify::init().unwrap();
        let cfg_file = ensure_cfg_file();
        let mut buf =
            vec![0; inotify::get_buffer_size(&std::path::Path::new(&cfg_file)).unwrap()];

        inotify
            .watches()
            .add(cfg_file, WatchMask::MODIFY)
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
