mod battery;
mod brightness;
mod keyboard;
#[macro_use]
mod netlink;
mod config;
mod notif;
mod sound;

use crate::config::Config;
use crate::notif::{Notification, Timeout, Urgency};
use knuffel::errors::Error as KnuffelError;
use std::collections::HashMap;
use std::os::unix::thread::JoinHandleExt;
use std::process::exit;
use std::sync::mpsc::Sender;
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

#[derive(Debug)]
pub enum Message {
    ModulePanic(String),
    ConfigReload(Config),
    ConfigReloadError(KnuffelError),
}

extern "C" fn sa_action(_: libc::c_int) {}

fn setup_sigaction(sender: Sender<Message>) {
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
            panic!("{}", errno_msg!("sigaction error"));
        }
    }

    std::panic::set_hook(Box::new(move |info| {
        let mut notif = Notification::new();
        let config = Config::get();
        let payload = info.payload();
        let try_send = |p| {
            if let Err(err) = sender.send(Message::ModulePanic(format!(
                "panic at '{}' - {p}\n{}",
                info.location().unwrap(), // blindly believing in rust docs that it won't ever panic
                std::backtrace::Backtrace::force_capture()
            ))) {
                println!("mpsc sender error: {err:#?}\npayload: {p}");
                exit(-1);
            };
        };

        notif
            .timeout(Timeout::Never)
            .urgency(Urgency::Critical)
            .summary("SUN just died")
            .body("Checks logs for details")
            .icon(&config.error_icon);
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

#[tokio::main]
async fn main() {
    let (sender, mut reciever) = tokio::sync::mpsc::unbounded_channel::<Message>();
    //let mut routines = HashMap::new();

    tokio::spawn(config::routine(sender.clone()));

    loop {
        if let Some(msg) = reciever.recv().await {
            dbg!(msg);
        }
    }
}
