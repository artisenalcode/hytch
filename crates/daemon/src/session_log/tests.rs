use super::{SessionLog, rotate_log_file};
use std::io::{Read, Seek, SeekFrom, Write};
use tempfile::NamedTempFile;

fn read_all(path: &std::path::Path) -> Vec<u8> {
    let mut f = std::fs::File::open(path).unwrap();
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).unwrap();
    buf
}

// --- rotate_log_file (pure, sync, called from a spawn_blocking task) -------

#[test]
fn rotate_leaves_undersized_file_untouched() {
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(b"short").unwrap();
    rotate_log_file(tmp.as_file_mut(), 100).unwrap();

    tmp.as_file_mut().seek(SeekFrom::Start(0)).unwrap();
    let mut buf = Vec::new();
    tmp.as_file_mut().read_to_end(&mut buf).unwrap();
    assert_eq!(buf, b"short");
}

#[test]
fn rotate_leaves_exactly_max_sized_file_untouched() {
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(b"1234").unwrap();
    rotate_log_file(tmp.as_file_mut(), 4).unwrap();

    tmp.as_file_mut().seek(SeekFrom::Start(0)).unwrap();
    let mut buf = Vec::new();
    tmp.as_file_mut().read_to_end(&mut buf).unwrap();
    assert_eq!(buf, b"1234");
}

#[test]
fn rotate_trims_oversized_file_to_the_tail() {
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(b"0123456789").unwrap(); // 10 bytes
    rotate_log_file(tmp.as_file_mut(), 4).unwrap();

    tmp.as_file_mut().seek(SeekFrom::Start(0)).unwrap();
    let mut buf = Vec::new();
    tmp.as_file_mut().read_to_end(&mut buf).unwrap();
    assert_eq!(buf, b"6789");
}

#[test]
fn rotate_leaves_cursor_at_eof_ready_for_append() {
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(b"0123456789").unwrap();
    rotate_log_file(tmp.as_file_mut(), 4).unwrap();

    tmp.as_file_mut().write_all(b"X").unwrap();
    tmp.as_file_mut().seek(SeekFrom::Start(0)).unwrap();
    let mut buf = Vec::new();
    tmp.as_file_mut().read_to_end(&mut buf).unwrap();
    assert_eq!(buf, b"6789X");
}

// --- SessionLog: open/append/rotate-at-cap/end-marker ----------------------

#[test]
fn session_log_open_creates_file_if_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mysession.log");
    assert!(!path.exists());

    let _log = SessionLog::open(&path, 1024).unwrap();
    assert!(path.exists());
}

#[test]
fn session_log_append_writes_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.log");
    let mut log = SessionLog::open(&path, 1024).unwrap();

    log.append(b"hello ").unwrap();
    log.append(b"world").unwrap();
    drop(log);

    assert_eq!(read_all(&path), b"hello world");
}

#[test]
fn session_log_rotates_automatically_once_cap_is_crossed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.log");
    let mut log = SessionLog::open(&path, 8).unwrap();

    log.append(b"01234567").unwrap(); // exactly at cap, no rotation yet
    log.append(b"89").unwrap(); // crosses cap -> rotates to last 8 bytes
    drop(log);

    assert_eq!(read_all(&path), b"23456789");
}

#[test]
fn session_log_reopen_trims_a_preexisting_oversized_file() {
    // Mirrors atch's open_log(): rotation also runs at open time, so a log
    // left oversized by a previous run (or a smaller -C on reopen) gets
    // trimmed immediately rather than waiting for the next write. Reopening
    // at cap 4 trims "0123456789" -> "6789"; appending "X" then pushes to
    // 5 bytes, which is > the 4-byte cap, so it rotates again to "789X".
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.log");
    std::fs::write(&path, b"0123456789").unwrap();

    let mut log = SessionLog::open(&path, 4).unwrap();
    log.append(b"X").unwrap();
    drop(log);

    assert_eq!(read_all(&path), b"789X");
}

#[test]
fn session_log_write_end_marker_appends_at_current_position() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.log");
    let mut log = SessionLog::open(&path, 1024).unwrap();

    log.append(b"output").unwrap();
    log.write_end_marker("[ended]").unwrap();
    drop(log);

    assert_eq!(read_all(&path), b"output[ended]");
}
