mod battery;
mod brightness;
mod keyboard;
#[macro_use]
mod netlink;
mod config;
mod notif;
mod sound;

use crate::config::Config;
use crate::notif::NotifWrapper;
use knuffel::errors::Error as KnuffelError;
use notify_rust::{Timeout, Urgency};
use std::collections::HashMap;
use std::os::unix::thread::JoinHandleExt;
use std::process::exit;
use std::thread::{spawn, JoinHandle};
use std::sync::mpsc::SyncSender;

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
    ConfigReload(Config),
    ConfigReloadError(KnuffelError),
}

extern "C" fn sa_action(_: libc::c_int) {
    dbg!("sa_action");
}

fn setup_sigaction(sender: SyncSender<Message>) {
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

    std::panic::set_hook(Box::new(move |info| {
        let mut notif = notif::NotifWrapper::new();
        let payload = info.payload();
        let try_send = |p| {
            if let Err(e) = sender.send(Message::ModulePanic(format!(
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
    let config = Config::update().unwrap();
    let (sender, reciever) = std::sync::mpsc::sync_channel::<Message>(1);
    let mut routines = HashMap::new();

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

    spawn(config::routine(sender.clone()));

    setup_sigaction(sender);

    loop {
        match reciever.recv() {
            Ok(Message::ConfigReload(config)) => {
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
            Ok(Message::ConfigReloadError(err)) => {
                NotifWrapper::new()
                    .summary("Config parse error")
                    .body("Check logs for details")
                    .urgency(Urgency::Critical)
                    .timeout(Timeout::Never)
                    .show()
                    .unwrap();
                println!("config parse error:\n{err:#?}");
            }
            Ok(Message::ModulePanic(payload)) => {
                println!("{payload}");
                break;
            }
            Err(e) => panic!("mpsc reciever error:\n{e:#?}"),
        }
    }
}
