# SUN (Some Useful Notifications)

Daemon for producing various notifications via [Freesektop Notifications](https://specifications.freedesktop.org/notification-spec/latest/)

All modules are hot reloadable via config file. You can turn them on and off or change any other property without restarting the application, just update config file and save it.

### Implemented modules:
1. Battery
- ##### Monitors `power_supply` events (charging, discharging, full, low) via netlink
- ##### Currently looks only for BAT0 (will add config to configure it)
2. Brightness
- ##### Monitors `backlight` events via netlink
- ##### Currently doesn't distinguish different gpu's (will add config to configure it)
3. Volume (libpulse + zbus)
- ##### Monitors default sink(headphones, speakers etc.) and sink(microphone)
- ##### Detects `org.bluez.Battery1` on bluetooth sink and polls it's capacity
4. Keyboard layout
- ##### Works with `X11` server shipped with `xkb` extension
- ##### Works with [Niri](https://github.com/YaLTeR/niri) via `NIRI_SOCKET`

## Notes:

App is tightly coupled with Linux (via netlink and sysfs).

All modules are running in separate thread each and if any of them will die main thread will exit too.
