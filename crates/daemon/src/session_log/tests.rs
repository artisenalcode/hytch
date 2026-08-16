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
fn rotate_does_not_split_an_unterminated_escape_sequence() {
    // "AB" + a cursor-position CSI sequence (ESC [ 1 2 3 G) + "XYZW". A
    // naive last-6-bytes cut lands as "3GXYZW" -- the escape sequence's
    // opening `ESC [ 1 2` is gone, so a terminal replaying this sees the
    // literal text "3GXYZW" instead of a cursor move, and (in the general
    // case, not this specific sequence) can end up consuming real
    // following bytes as bogus parameters. The fix must widen the cut left
    // to the `ESC` byte so the whole sequence survives intact.
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(b"AB\x1b[123GXYZW").unwrap(); // 12 bytes
    rotate_log_file(tmp.as_file_mut(), 6).unwrap();

    assert_eq!(read_all(tmp.path()), b"\x1b[123GXYZW");
}

#[test]
fn rotate_does_not_split_a_multibyte_utf8_character() {
    // "AB" + the 3-byte UTF-8 box-drawing char U+2500 (0xe2 0x94 0x80) +
    // "XYZW". A naive last-6-bytes cut starts one byte into that
    // character (a continuation byte), which is invalid UTF-8 on its own
    // and renders as replacement-character mojibake. The fix must skip
    // forward to the start of the next full character instead.
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all("AB\u{2500}XYZW".as_bytes()).unwrap(); // 2 + 3 + 4 = 9 bytes
    rotate_log_file(tmp.as_file_mut(), 6).unwrap();

    let out = read_all(tmp.path());
    assert!(
        std::str::from_utf8(&out).is_ok(),
        "trimmed tail must be valid UTF-8, got {out:?}"
    );
    assert_eq!(out, "XYZW".as_bytes());
}

#[test]
fn rotate_with_no_unsafe_boundary_still_cuts_exactly_at_the_cap() {
    // Baseline: when the naive cut point doesn't land inside anything
    // unsafe, behavior is unchanged from a plain byte-offset trim -- the
    // safety logic shouldn't widen cuts that don't need it.
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(b"0123456789").unwrap();
    rotate_log_file(tmp.as_file_mut(), 4).unwrap();

    assert_eq!(read_all(tmp.path()), b"6789");
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
