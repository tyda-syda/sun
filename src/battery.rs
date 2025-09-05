use crate::config::Config;
use crate::netlink::utils as ev_utils;
use crate::netlink::{NetlinkError, NetlinkHandle, Uevent};
use crate::notif::NotifWrapper;
use notify_rust::Urgency;
use std::fs;
use std::io::ErrorKind;
use std::str::FromStr;

const SYS_PATH: &'static str = "/sys/class/power_supply/{name}/uevent";

#[derive(Debug, PartialEq, Eq, Clone)]
enum Status {
    Charging,
    Discharging,
    Full,
    Unknown(String),
}

struct UeventPowerSupply {
    status: Status,
    capacity: u8,
}

impl UeventPowerSupply {
    pub fn new() -> Result<Self, String> {
        let uevent_str =
            fs::read_to_string(SYS_PATH.replace("{name}", &Config::get().battery.target))
                .map_err(|e| e.to_string())?;
        let status = ev_utils::get_element_val(&uevent_str, "POWER_SUPPLY_STATUS")
            .ok_or("POWER_SUPPLY_STATUS missing".to_owned())?
            .into();

        if let Some(capacity) = ev_utils::get_element_val(&uevent_str, "POWER_SUPPLY_CAPACITY") {
            Ok(Self {
                status,
                capacity: u8::from_str(&capacity).map_err(|err| err.to_string())?,
            })
        } else {
            let now = ev_utils::get_element_val(&uevent_str, "POWER_SUPPLY_ENERGY_NOW")
                .ok_or("POWER_SUPPLY_ENERGY_NOW missing".to_owned())
                .map(|now| f32::from_str(&now))?
                .map_err(|err| err.to_string())?;
            let full = ev_utils::get_element_val(&uevent_str, "POWER_SUPPLY_ENERGY_FULL")
                .ok_or("POWER_SUPPLY_ENERGY_FULL missing".to_owned())
                .map(|now| f32::from_str(&now))?
                .map_err(|err| err.to_string())?;

            Ok(Self {
                status,
                capacity: (now / full * 100.) as u8,
            })
        }
    }
}

impl Uevent<String> for UeventPowerSupply {
    fn from_bytes(data: &Vec<u8>) -> Result<Self, String> {
        let uevent_str =
            String::from_utf8(data.clone()).map_err(|_| String::from("not valid utf8"))?;

        if !uevent_str.contains("SUBSYSTEM=power_supply") {
            return Err("non power_supply".into());
        }

        // from netlink we only receive notification that battery has changed
        // all info we will read from sysfs
        Self::new()
    }
}

impl From<&str> for Status {
    fn from(value: &str) -> Self {
        if value == "Charging" {
            Status::Charging
        } else if value == "Discharging" {
            Status::Discharging
        } else if value == "Full" {
            Status::Full
        } else {
            Status::Unknown(value.into())
        }
    }
}

impl From<String> for Status {
    fn from(value: String) -> Self {
        <Self as From<&str>>::from(&value)
    }
}

impl ToString for Status {
    fn to_string(&self) -> String {
        match self {
            Status::Discharging => "Discharging".into(),
            Status::Charging => "Charging".into(),
            Status::Full => "Full".into(),
            Status::Unknown(val) => val.into(),
        }
    }
}

pub fn routine() -> impl crate::Routine {
    || {
        let mut handle = NetlinkHandle::new().unwrap();
        let mut notif = NotifWrapper::new();
        let mut last_status = UeventPowerSupply::new().unwrap().status;
        let mut poll_timeout = Config::get().battery.poll_timeout;
        let mut full = false;

        loop {
            let config_battery = Config::get().battery;

            if config_battery.off {
                break;
            }

            notif.summary("Battery").icon(&config_battery.icon_path);

            match handle.read_uevent_msec::<UeventPowerSupply, String>(poll_timeout) {
                Ok(ev) => {
                    if ev.status == last_status {
                        continue;
                    }

                    full = false;
                    poll_timeout = config_battery.poll_timeout;
                    last_status = ev.status;

                    notif.hints.clear(); // prevents from setting multiple urgencies
                    notif
                        .urgency(Urgency::Normal)
                        .body(last_status.to_string().as_str())
                        .timeout(2500);

                    let level = format!("{}0", std::cmp::max(ev.capacity / 10, 1));
                    let icon = match last_status {
                        Status::Discharging => {
                            if config_battery.dynamic_discharging_icon {
                                config_battery.discharging_icon.replace("{level}", &level)
                            } else {
                                config_battery.discharging_icon
                            }
                        }
                        Status::Charging => {
                            if config_battery.dynamic_charging_icon {
                                config_battery.charging_icon.replace("{level}", &level)
                            } else {
                                config_battery.charging_icon
                            }
                        }
                        Status::Full => {
                            full = true;
                            poll_timeout = -1; // wait for uevent, no need to poll for now
                            config_battery.full_icon
                        }
                        Status::Unknown(ref status) => {
                            println!("unknown battery status: {status}");
                            continue;
                        }
                    };

                    notif.icon += &icon;
                    notif.show();
                }
                Err(NetlinkError::Timeout) => {
                    let uevent = UeventPowerSupply::new().unwrap();

                    notif.body(last_status.to_string().as_str()).timeout(0);

                    if !full && uevent.status == Status::Full {
                        full = true;
                        poll_timeout = -1; // wait for uevent, no need to poll for now

                        notif.urgency(Urgency::Normal);
                        notif.body("Battery is full");
                        notif.icon += &config_battery.full_icon;
                        notif.show();

                        continue;
                    }

                    let cap = uevent.capacity;

                    if uevent.status == Status::Discharging && cap <= config_battery.warn_at {
                        notif.urgency(Urgency::Critical);
                        notif.body(format!("{cap}% left, connect charger").as_str());
                        notif.icon += &config_battery.low_icon;
                        notif.show();
                    }
                }
                Err(NetlinkError::IO(ErrorKind::Interrupted)) => (),
                Err(NetlinkError::IO(kind)) => panic!("{kind:?}"),
                Err(_) => (),
            }
        }
    }
}
