use crate::config::Config;
use crate::netlink::utils as ev_utils;
use crate::netlink::{NetlinkError, NetlinkHandle, Uevent};
use crate::notif::NotifWrapper;
use notify_rust::Hint;
use std::io::ErrorKind;
use std::str::FromStr;

struct UeventBacklight {
    devpath: String,
}

impl Uevent<String> for UeventBacklight {
    fn from_bytes(data: &Vec<u8>) -> Result<Self, String> {
        let uevent_str =
            String::from_utf8(data.clone()).map_err(|_| String::from("invalid utf8"))?;

        if !uevent_str.contains("SUBSYSTEM=backlight") {
            return Err("non backlight".into());
        }

        Ok(Self {
            devpath: ev_utils::get_element_val(&uevent_str, "@")
                .ok_or(String::from("devpath not found"))?,
        })
    }
}

impl UeventBacklight {
    fn get_sys_val(&self, name: &str) -> f32 {
        let val = std::fs::read(format!("/sys{}/{}", self.devpath, name))
            .unwrap()
            .iter()
            .take_while(|b| **b != b'\n')
            .map(|b| *b as char)
            .collect::<String>();

        f32::from_str(&val).unwrap()
    }

    fn get_brightness(&self) -> u32 {
        (self.get_sys_val("brightness") / self.get_sys_val("max_brightness") * 100.) as u32
    }
}

pub fn routine() -> impl crate::Routine {
    || {
        let mut last_brightness = 0; // TODO: replace with actual value
        let mut handle = NetlinkHandle::new().unwrap();
        let mut notif = NotifWrapper::new();

        loop {
            let brightness_config = Config::get().brightness;

            if brightness_config.off {
                dbg!("brightness module disabled");
                break;
            }

            match handle.read_uevent::<UeventBacklight, String>() {
                Ok(ev) => {
                    if last_brightness == ev.get_brightness() {
                        continue;
                    }

                    last_brightness = ev.get_brightness();

                    notif.summary("Brightness")
                        .icon(&format!("{}{}", brightness_config.icon_path, brightness_config.icon))
                        .timeout(3000)
                        .hint(Hint::CustomInt("value".into(), last_brightness as i32));
                    notif.show();
                }
                Err(NetlinkError::IO(ErrorKind::Interrupted))
                | Err(NetlinkError::Serialize(_))
                | Err(NetlinkError::Timeout) => (),
                Err(NetlinkError::IO(kind)) => panic!("{kind:?}"),
            }
        }
    }
}
