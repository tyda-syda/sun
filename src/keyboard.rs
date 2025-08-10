use crate::notif::NotifWrapper;
use serde_json;
use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use xcb::xkb;

mod niri {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, Debug)]
    pub(super) enum Response {
        KeyboardLayoutsChanged(KeyboardLayoutsChanged),
        KeyboardLayoutSwitched(KeyboardLayoutSwitched),
    }

    #[derive(Serialize, Deserialize, Debug)]
    pub(super) struct KeyboardLayoutsChanged {
        pub keyboard_layouts: Layouts,
    }

    #[derive(Serialize, Deserialize, Debug)]
    pub(super) struct Layouts {
        pub names: Vec<String>,
        pub current_idx: u8,
    }

    #[derive(Serialize, Deserialize, Debug)]
    pub(super) struct KeyboardLayoutSwitched {
        pub idx: u8,
    }
}

fn try_x11() -> Option<impl FnMut() -> String> {
    let conn = xcb::Connection::connect_with_extensions(None, &[xcb::Extension::Xkb], &[])
        .ok()?
        .0;

    if !conn
        .wait_for_reply(conn.send_request(&xkb::UseExtension {
            wanted_major: 1,
            wanted_minor: 0,
        }))
        .ok()?
        .supported()
    {
        return None;
    }

    let core_kbd = xkb::Id::UseCoreKbd as u16;

    conn.check_request(conn.send_request_checked(&xkb::SelectEvents {
        device_spec: core_kbd,
        affect_which: xkb::EventType::STATE_NOTIFY,
        clear: xkb::EventType::empty(),
        select_all: xkb::EventType::STATE_NOTIFY,
        affect_map: xkb::MapPart::empty(),
        map: xkb::MapPart::empty(),
        details: &[],
    }))
    .unwrap();

    let mut current_group = conn
        .wait_for_reply(conn.send_request(&xkb::GetState {
            device_spec: core_kbd,
        }))
        .unwrap()
        .group();

    let layout = move || loop {
        break match conn.wait_for_event() {
            Ok(xcb::Event::Xkb(xkb::Event::StateNotify(state))) => {
                if state.group() == current_group {
                    continue;
                }

                current_group = state.group();

                conn.wait_for_reply(conn.send_request(&xkb::GetNames {
                    device_spec: core_kbd,
                    which: xkb::NameDetail::GROUP_NAMES,
                }))
                .unwrap()
                .value_list()
                .iter()
                .filter_map(|val| match val {
                    xkb::GetNamesReplyValueList::GroupNames(atoms) => Some(atoms),
                    _ => None,
                })
                .flat_map(|atoms| atoms)
                .map(|atom| {
                    conn.wait_for_reply(conn.send_request(&xcb::x::GetAtomName { atom: *atom }))
                        .unwrap()
                        .name()
                        .as_ascii()
                        .to_owned()
                })
                .nth(current_group as usize)
                .unwrap()
            }
            Ok(_) => {
                continue;
            }
            Err(err) => {
                println!("xcb recieved error event: {err:?}");
                continue;
            }
        };
    };

    Some(layout)
}

fn try_niri() -> Option<impl FnMut() -> String> {
    let mut sock = UnixStream::connect(std::env::var("NIRI_SOCKET").ok()?).ok()?;
    let mut buf_reader = BufReader::new(sock.try_clone().unwrap());
    let mut buf = String::new();
    let mut layouts = Vec::new();

    sock.write_all(b"\"EventStream\"\n").unwrap();
    sock.shutdown(Shutdown::Write).unwrap();
    buf_reader.read_line(&mut buf).unwrap(); // discard OK reponse

    let layout = move || loop {
        buf.clear();
        buf_reader.read_line(&mut buf).unwrap();

        break match serde_json::from_str::<niri::Response>(&buf) {
            Ok(niri::Response::KeyboardLayoutsChanged(niri::KeyboardLayoutsChanged {
                keyboard_layouts,
                ..
            })) => {
                layouts.clear();
                layouts.extend_from_slice(&keyboard_layouts.names);
                continue;
            }
            Ok(niri::Response::KeyboardLayoutSwitched(niri::KeyboardLayoutSwitched { idx })) => {
                layouts[idx as usize].clone()
            }
            Err(_) => continue,
        };
    };

    Some(layout)
}

pub fn routine() -> impl crate::Routine {
    || {
        let mut notif = NotifWrapper::new();
        let mut layout: Box<dyn FnMut() -> String>;

        if let Some(niri_layout) = try_niri() {
            layout = Box::new(niri_layout);
        } else {
            layout = Box::new(try_x11().expect("neither niri nor X11 with KBD found"));
        };

        loop {
            notif
                .timeout(2500)
                .summary("Layout")
                .body(&layout())
                .icon("/usr/share/icons/Adwaita/symbolic/devices/input-keyboard-symbolic.svg");
            notif.show();
        }
    }
}
