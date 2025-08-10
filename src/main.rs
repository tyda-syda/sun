mod battery;
mod brightness;
mod keyboard;
mod netlink;
mod notif;
mod sound;

use std::process::exit;
use std::thread::spawn;
use notify_rust::Urgency;

// workaround for type aliases, example:
// type Routine = impl FnOnce() + Send + 'static - won't compile
trait Routine: FnOnce() + Send + 'static {}

impl<T: FnOnce() + Send + 'static> Routine for T {}

fn main() {
    let (sender, reciever) = std::sync::mpsc::sync_channel::<String>(1);

    std::panic::set_hook(Box::new(move |info| {
        let mut notif = notif::NotifWrapper::new();
        let payload = info.payload();
        let try_send = |p| {
            if let Err(e) = sender.send(format!(
                "panic at '{}' - {p}\n{}",
                info.location().unwrap(), // blindly believing in rust docs that it won't ever panic
                std::backtrace::Backtrace::force_capture()
            )) {
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

    spawn(brightness::routine());
    spawn(keyboard::routine());
    spawn(battery::routine());
    spawn(sound::routine());

    match reciever.recv() {
        Ok(v) => println!("{v}"),
        Err(e) => println!("mpsc reciever error: {e:?}"),
    }
}
