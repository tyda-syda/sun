use crate::config::Config;
use crate::notif::NotifWrapper;
use serde_json;
use std::io::{BufRead, BufReader, Error, ErrorKind, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use xcb::xkb;

type LayoutFunc = Box<dyn FnMut() -> Result<String, Error>>;

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

fn map_xcb_err(err: xcb::Error) -> Error {
    match err {
        xcb::Error::Connection(xcb::ConnError::Connection) => Error::last_os_error().into(),
        xcb::Error::Connection(err) => {
            Error::new(ErrorKind::Other, format!("xcb connection err: {err:#?}"))
        }
        xcb::Error::Protocol(err) => {
            Error::new(ErrorKind::Other, format!("xcb protocol err: {err:#?}"))
        }
    }
}

fn x11() -> Option<LayoutFunc> {
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
    .ok()?;

    let mut current_group = conn
        .wait_for_reply(conn.send_request(&xkb::GetState {
            device_spec: core_kbd,
        }))
        .ok()?
        .group();

    let func = move || loop {
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
                .map_err(map_xcb_err)?
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
                .map(|layout| Ok(layout))
                .unwrap()
            }
            Ok(_) => {
                continue;
            }
            Err(err) => Err(map_xcb_err(err)),
        };
    };

    Some(Box::new(func))
}

fn niri() -> Option<LayoutFunc> {
    let mut sock = UnixStream::connect(std::env::var("NIRI_SOCKET").ok()?).ok()?;
    let mut buf_reader = BufReader::new(sock.try_clone().unwrap());
    let mut layouts = Vec::new();

    sock.write_all(b"\"EventStream\"\n").unwrap();
    sock.shutdown(Shutdown::Write).unwrap();
    buf_reader.read_line(&mut String::new()).unwrap(); // discard OK reponse

    let func = move || loop {
        // do not use BufReader::read_line() here
        // it ignores EINTR inside of BufReader::read_until()
        let (msg, num) = 'outer: loop {
            let msg = buf_reader.fill_buf().map_err(|e| e.kind())?;

            for idx in 0..msg.len() {
                if msg[idx] == b'\n' {
                    break 'outer (String::from_utf8(Vec::from(&msg[..idx])).unwrap(), idx + 1);
                }
            }
        };

        buf_reader.consume(num);

        break match serde_json::from_str::<niri::Response>(&msg) {
            Ok(niri::Response::KeyboardLayoutsChanged(niri::KeyboardLayoutsChanged {
                keyboard_layouts,
                ..
            })) => {
                layouts.clear();
                layouts.extend_from_slice(&keyboard_layouts.names);
                continue;
            }
            Ok(niri::Response::KeyboardLayoutSwitched(niri::KeyboardLayoutSwitched { idx })) => {
                Ok(layouts[idx as usize].clone())
            }
            Err(_) => continue, // ignore non keyboard related events
        };
    };

    Some(Box::new(func))
}

fn layout_provider() -> LayoutFunc {
    if let Some(niri_layout) = niri() {
        return niri_layout;
    };

    if let Some(x11_layout) = x11() {
        return x11_layout;
    };

    panic!("neither niri nor X11 with KBD found");
}

pub fn routine() -> impl crate::Routine {
    || {
        let mut notif = NotifWrapper::new();
        let mut get_layout = layout_provider();

        loop {
            let keyboard_config = Config::get().keyboard;

            if keyboard_config.off {
                dbg!("keyboard module disabled");
                break;
            }

            let layout = match get_layout() {
                Ok(layout) => layout,
                Err(err) if matches!(err.kind(), ErrorKind::Interrupted) => continue,
                Err(err) => panic!("{err:#?}"),
            };

            notif
                .timeout(2500)
                .summary("Layout")
                .body(&layout)
                .icon(&format!("{}{}", keyboard_config.icon_path, keyboard_config.icon));
            notif.show();
        }
    }
}
