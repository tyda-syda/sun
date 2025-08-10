# SUN (Some Useful Notifications)

Daemon for producing various notifications via [Freesektop Notifications](https://specifications.freedesktop.org/notification-spec/latest/)

### Current implemented notification modules:
1. Battery
2. Brightness
3. Volume (with bluez support)
4. Keyboard layout (X11 and Niri)

App is tightly coupled with Linux (via netlink and sysfs). I didn't use any production ready runtime like Tokio deliberately. All modules are running in separate thread each and if any of them will die main thread will exit too.

### TODO list:
1. Add configurations for each module (make it hot reload)
2. Cleanup backtrace on child thread panic (make it less verbose, too much unneeded info prints for now)
3. Remove all hardcoded values like icon pathes and poll timeouts, extract them into configuration
4. Add callback functionality, for example to notify notification bar when battery is charging now (if it can't check by itself)
5. Add support for OpenBSD maybe?
