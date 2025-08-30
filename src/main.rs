mod battery;
mod brightness;
mod keyboard;
#[macro_use]
mod netlink;
mod config;
mod notif;
mod sound;

use notify_rust::Urgency;
use std::collections::HashMap;
use std::os::unix::thread::JoinHandleExt;
use std::process::exit;
use std::thread::{spawn, JoinHandle};

// workaround for type aliases, example:
// type Routine = impl FnOnce() + Send + 'static - won't compile
trait Routine: FnOnce() + Send + 'static {}

impl<T: FnOnce() + Send + 'static> Routine for T {}

#[derive(Eq, PartialEq, Hash)]
pub enum Module {
    Sound,
    Battery,
    Brightness,
    Keyboard,
}

pub enum Message {
    ModulePanic(String),
    ConfigReload,
}

extern "C" fn sa_action(_: libc::c_int) {
    dbg!("sa_action");
}

fn update_routine(
    name: Module,
    routines: &mut HashMap<Module, JoinHandle<()>>,
    off: bool,
    routine: impl Routine,
) {
    if let Some(handle) = routines.get_mut(&name) {
        unsafe {
            if libc::pthread_kill(handle.as_pthread_t(), libc::SIGUSR1) != 0 {
                println!("{}", errno_msg!("pthread_kill error"));
                exit(-1);
            }
        }

        if off {
            routines.remove(&name).unwrap().join().unwrap();
        }
    } else {
        if !off {
            routines.insert(name, spawn(routine));
        }
    }
}

fn main() {
    let (sender, reciever) = std::sync::mpsc::sync_channel::<Message>(1);
    let hook_sender = sender.clone();

    config::Config::update();

    std::panic::set_hook(Box::new(move |info| {
        let mut notif = notif::NotifWrapper::new();
        let payload = info.payload();
        let try_send = |p| {
            if let Err(e) = hook_sender.send(Message::ModulePanic(format!(
                "panic at '{}' - {p}\n{}",
                info.location().unwrap(), // blindly believing in rust docs that it won't ever panic
                std::backtrace::Backtrace::force_capture()
            ))) {
                println!("mpsc sender error: {e:?}\npayload: {p}");
                exit(-1);
            };
        };

        notif
            .timeout(0)
            .urgency(Urgency::Critical)
            .summary("SUN just died")
            .body("Checks logs for details")
            .icon("/usr/share/icons/Adwaita/symbolic/status/computer-fail-symbolic.svg");
        notif.show();

        if payload.is::<String>() {
            try_send(payload.downcast_ref::<String>().unwrap().clone());
        } else if payload.is::<&str>() {
            try_send(String::from(*payload.downcast_ref::<&str>().unwrap()));
        } else {
            // not possible according to rust docs, but just in case...
            try_send(String::from("unknown panic payload type, exiting..."));
        }
    }));

    unsafe {
        let mut action = std::mem::zeroed::<libc::sigaction>();

        action.sa_sigaction = sa_action as usize;
        action.sa_flags = libc::SA_NODEFER;

        if libc::sigaction(
            libc::SIGUSR1,
            &action as *const libc::sigaction,
            std::ptr::null_mut(),
        ) == -1
        {
            panic!("sigaction err");
        }
    }

    let mut routines = HashMap::new();
    let config = config::Config::get();

    if !config.sound.off {
        routines.insert(Module::Sound, spawn(sound::routine()));
    }

    if !config.battery.off {
        routines.insert(Module::Battery, spawn(battery::routine()));
    }

    if !config.keyboard.off {
        routines.insert(Module::Keyboard, spawn(keyboard::routine()));
    }

    if !config.brightness.off {
        routines.insert(Module::Brightness, spawn(brightness::routine()));
    }

    spawn(config::routine(sender));

    loop {
        match reciever.recv() {
            Ok(Message::ConfigReload) => {
                let config = config::Config::get();

                update_routine(
                    Module::Sound,
                    &mut routines,
                    config.sound.off,
                    sound::routine(),
                );
                update_routine(
                    Module::Battery,
                    &mut routines,
                    config.battery.off,
                    battery::routine(),
                );
                update_routine(
                    Module::Keyboard,
                    &mut routines,
                    config.keyboard.off,
                    keyboard::routine(),
                );
                update_routine(
                    Module::Brightness,
                    &mut routines,
                    config.brightness.off,
                    brightness::routine(),
                );
            }
            Ok(Message::ModulePanic(payload)) => {
                println!("{payload}");
                break;
            }
            Err(e) => panic!("mpsc reciever error: {e:#?}"),
        }
    }
}
