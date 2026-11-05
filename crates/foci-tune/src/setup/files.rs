//! Contents of the udev rule and the systemd unit that `setup` installs.

use std::path::Path;

use foci_trace::usb::{TRACE_PRODUCT_ID, TRACE_VENDOR_ID};

/// Group the udev rule grants access to the trace device.
pub(crate) const GROUP: &str = "plugdev";

/// Unit name `systemctl` is given.
pub(crate) const SERVICE: &str = "foci-tune";

const USER_PLACEHOLDER: &str = "@USER@";
const LAUNCHER_PLACEHOLDER: &str = "@LAUNCHER@";

const UNIT_TEMPLATE: &str = "\
[Unit]
Description=foci-tune tuning service
After=network.target moonraker.service
Wants=moonraker.service

[Service]
Type=simple
User=@USER@
ExecStartPre=@LAUNCHER@ --check
ExecStart=@LAUNCHER@ --moonraker-url http://127.0.0.1:7125 --listen 0.0.0.0:7130
Restart=on-failure
RestartSec=5
NoNewPrivileges=true

[Install]
WantedBy=multi-user.target
";

/// The udev rule giving [`GROUP`] access to the FOCI trace USB device.
pub(crate) fn udev_rule() -> String {
    format!(
        "SUBSYSTEM==\"usb\", ATTR{{idVendor}}==\"{TRACE_VENDOR_ID:04x}\", \
         ATTR{{idProduct}}==\"{TRACE_PRODUCT_ID:04x}\", MODE=\"0660\", GROUP=\"{GROUP}\"\n"
    )
}

/// The systemd unit running the service as `user` through the `launcher` symlink.
pub(crate) fn unit(user: &str, launcher: &Path) -> String {
    UNIT_TEMPLATE
        .replace(USER_PLACEHOLDER, user)
        .replace(LAUNCHER_PLACEHOLDER, &launcher.to_string_lossy())
}
