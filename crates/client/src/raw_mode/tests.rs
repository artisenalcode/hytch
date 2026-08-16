use super::RawModeGuard;
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{LocalModes, tcgetattr};

/// Allocate a real pty and return (master, slave) -- termios operations
/// need a real tty device, which a plain pipe isn't. The master must stay
/// alive for the slave's termios ops to work: once the master side is
/// closed, the kernel treats the slave as orphaned and termios calls on it
/// start failing with EIO.
fn open_pty_pair() -> (rustix::fd::OwnedFd, std::fs::File) {
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
    grantpt(&master).unwrap();
    unlockpt(&master).unwrap();
    let slave_name = ptsname(&master, Vec::new()).unwrap();
    let slave_path: std::path::PathBuf = slave_name.to_string_lossy().into_owned().into();
    let slave = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&slave_path)
        .unwrap();
    (master, slave)
}

#[test]
fn enable_switches_to_raw_mode_and_drop_restores_original() {
    let (_master, slave) = open_pty_pair();

    let before = tcgetattr(&slave).unwrap();
    assert!(
        before.local_modes.contains(LocalModes::ICANON),
        "a freshly opened pty slave should start in canonical mode"
    );
    assert!(
        before.local_modes.contains(LocalModes::ECHO),
        "a freshly opened pty slave should start with echo on"
    );

    {
        let guard = RawModeGuard::enable(&slave).unwrap();
        let raw = tcgetattr(&slave).unwrap();
        assert!(
            !raw.local_modes.contains(LocalModes::ICANON),
            "raw mode must disable canonical (line-buffered) input"
        );
        assert!(
            !raw.local_modes.contains(LocalModes::ECHO),
            "raw mode must disable local echo -- the daemon/program owns echoing"
        );
        assert_eq!(guard.original().local_modes, before.local_modes);
    } // guard dropped here

    let after = tcgetattr(&slave).unwrap();
    assert_eq!(
        after.local_modes, before.local_modes,
        "dropping the guard must restore the original settings"
    );
}
