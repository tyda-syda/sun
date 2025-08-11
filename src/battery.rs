use crate::netlink::utils as ev_utils;
use crate::netlink::{NetlinkError, NetlinkHandle, Uevent};
use crate::notif::NotifWrapper;
use notify_rust::Urgency;
use std::fs;
use std::str::FromStr;

const SYS_PATH: &'static str = "/sys/class/power_supply/BAT0";

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
            &fs::read_to_string(format!("{SYS_PATH}/uevent")).map_err(|e| e.to_string())?;
        let mut capacity = ev_utils::get_element_val(&uevent_str, "POWER_SUPPLY_CAPACITY")
            .ok_or("POWER_SUPPLY_CAPACITY missing".to_owned())
            .map(|cap| u8::from_str(&cap).map_err(|err| err.to_string()));

        if matches!(capacity, Err(_)) {
            let now = ev_utils::get_element_val(&uevent_str, "POWER_SUPPLY_ENERGY_NOW")
                .ok_or("POWER_SUPPLY_ENERGY_NOW missing".to_owned())
                .map(|now| f32::from_str(&now))?
                .map_err(|err| err.to_string())?;
            let full = ev_utils::get_element_val(&uevent_str, "POWER_SUPPLY_ENERGY_FULL")
                .ok_or("POWER_SUPPLY_ENERGY_FULL missing".to_owned())
                .map(|now| f32::from_str(&now))?
                .map_err(|err| err.to_string())?;

            capacity = Ok(Ok((now / full * 100.) as u8));
        }

        Ok(Self {
            status: ev_utils::get_element_val(&uevent_str, "POWER_SUPPLY_STATUS")
                .ok_or("POWER_SUPPLY_STATUS missing".to_owned())?
                .into(),
            capacity: capacity??,
        })
    }
}

impl Uevent<String> for UeventPowerSupply {
    fn from_bytes(data: &Vec<u8>) -> Result<Self, String> {
        let uevent_str =
            String::from_utf8(data.clone()).map_err(|_| String::from("not valid utf8"))?;

        if !uevent_str.contains("SUBSYSTEM=power_supply") {
            return Err("non power_supply".into());
        }

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
        let mut last_status = UeventPowerSupply::new().unwrap().status;
        let mut full = false;
        let mut notif = NotifWrapper::new();

        loop {
            match handle.read_uevent_msec::<UeventPowerSupply, String>(15 * 1000) {
                Ok(ev) => {
                    if ev.status == last_status {
                        continue;
                    }

                    full = false;
                    last_status = ev.status;

                    notif
                        .summary("Battery")
                        .body(last_status.to_string().as_str())
                        .urgency(Urgency::Normal)
                        .timeout(2500)
                        .icon("/usr/share/icons/Adwaita/symbolic/status/");

                    notif.icon += match last_status {
                        Status::Discharging => "battery-level-30-symbolic.svg",
                        Status::Charging => "battery-level-30-charging-symbolic.svg",
                        _ => {
                            println!("unknown battery status: {last_status:?}");
                            continue;
                        }
                    };

                    notif.show();
                }
                Err(NetlinkError::Timeout) => {
                    let uevent = UeventPowerSupply::new().unwrap();

                    notif
                        .summary("Battery")
                        .body(last_status.to_string().as_str())
                        .urgency(Urgency::Normal)
                        .timeout(0)
                        .icon("/usr/share/icons/Adwaita/symbolic/status/");

                    if !full && uevent.status == Status::Full {
                        full = true;

                        notif.body("Battery is full");
                        notif.icon += "battery-level-100-charged-symbolic.svg";
                        notif.show();

                        continue;
                    }

                    let cap = uevent.capacity;

                    if uevent.status == Status::Discharging && cap <= 15 {
                        notif.body(format!("{cap}% left, connect charger").as_str());
                        notif.urgency(Urgency::Critical);
                        notif.icon += "battery-caution-symbolic.svg";
                        notif.show();
                    }
                }
                Err(NetlinkError::IO(kind)) => panic!("{kind:?}"),
                Err(_) => (),
            }
        }
    }
}
