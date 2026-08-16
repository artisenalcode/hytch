use crate::message::{Message, RedrawMethod};
use bytes::{Buf, BufMut, BytesMut};
use std::io;
use tokio_util::codec::{Decoder, Encoder};

/// Bounds how large a single frame's claimed payload length may be before
/// decode() rejects it outright, without waiting for (or allocating) that
/// much data. Protects against a hostile or corrupt peer driving unbounded
/// memory use — the C protocol had no such guard because its 8-byte cap made
/// the question moot; framing a real payload size means we need one.
pub const DEFAULT_MAX_FRAME_LEN: usize = 64 * 1024;

/// 1 byte message-type tag + 4 byte big-endian payload length.
const HEADER_LEN: usize = 5;

const TYPE_PUSH: u8 = 0;
const TYPE_ATTACH: u8 = 1;
const TYPE_DETACH: u8 = 2;
const TYPE_WINCH: u8 = 3;
const TYPE_REDRAW: u8 = 4;
const TYPE_KILL: u8 = 5;

/// Length-prefixed framing for [`Message`] over the client↔daemon control
/// socket. Fully buffers partial reads/writes (via [`Decoder`]/[`Encoder`]),
/// so a short `read()` — which the C client_activity() treated as a fatal
/// protocol error — is just "not enough data yet, try again."
pub struct MessageCodec {
    max_frame_len: usize,
}

impl Default for MessageCodec {
    fn default() -> Self {
        MessageCodec {
            max_frame_len: DEFAULT_MAX_FRAME_LEN,
        }
    }
}

impl MessageCodec {
    pub fn new(max_frame_len: usize) -> Self {
        MessageCodec { max_frame_len }
    }
}

impl Encoder<Message> for MessageCodec {
    type Error = io::Error;

    fn encode(&mut self, msg: Message, dst: &mut BytesMut) -> Result<(), Self::Error> {
        match msg {
            Message::Push(payload) => {
                write_header(dst, TYPE_PUSH, payload.len());
                dst.extend_from_slice(&payload);
            }
            Message::Attach { skip_ring } => {
                write_header(dst, TYPE_ATTACH, 1);
                dst.put_u8(skip_ring as u8);
            }
            Message::Detach => write_header(dst, TYPE_DETACH, 0),
            Message::Winch { rows, cols } => {
                write_header(dst, TYPE_WINCH, 4);
                dst.put_u16(rows);
                dst.put_u16(cols);
            }
            Message::Redraw { method, rows, cols } => {
                write_header(dst, TYPE_REDRAW, 5);
                dst.put_u8(method.to_u8());
                dst.put_u16(rows);
                dst.put_u16(cols);
            }
            Message::Kill { signal } => {
                write_header(dst, TYPE_KILL, 1);
                dst.put_u8(signal);
            }
        }
        Ok(())
    }
}

fn write_header(dst: &mut BytesMut, ty: u8, len: usize) {
    dst.put_u8(ty);
    dst.put_u32(len as u32);
}

impl Decoder for MessageCodec {
    type Item = Message;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Message>, Self::Error> {
        if src.len() < HEADER_LEN {
            return Ok(None);
        }

        let ty = src[0];
        let len = u32::from_be_bytes([src[1], src[2], src[3], src[4]]) as usize;

        if len > self.max_frame_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame length {len} exceeds max {}", self.max_frame_len),
            ));
        }

        if src.len() < HEADER_LEN + len {
            // Not fatal — just don't have the whole frame yet. Reserve so
            // the next read() has room without another realloc.
            src.reserve(HEADER_LEN + len - src.len());
            return Ok(None);
        }

        let mut frame = src.split_to(HEADER_LEN + len);
        frame.advance(HEADER_LEN);
        decode_payload(ty, frame).map(Some)
    }
}

fn decode_payload(ty: u8, mut payload: BytesMut) -> io::Result<Message> {
    match ty {
        TYPE_PUSH => Ok(Message::Push(payload.freeze())),
        TYPE_ATTACH => {
            require_len(&payload, 1)?;
            Ok(Message::Attach {
                skip_ring: payload[0] != 0,
            })
        }
        TYPE_DETACH => {
            require_len(&payload, 0)?;
            Ok(Message::Detach)
        }
        TYPE_WINCH => {
            require_len(&payload, 4)?;
            Ok(Message::Winch {
                rows: payload.get_u16(),
                cols: payload.get_u16(),
            })
        }
        TYPE_REDRAW => {
            require_len(&payload, 5)?;
            let method_byte = payload.get_u8();
            let method = RedrawMethod::from_u8(method_byte).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unknown redraw method {method_byte}"),
                )
            })?;
            Ok(Message::Redraw {
                method,
                rows: payload.get_u16(),
                cols: payload.get_u16(),
            })
        }
        TYPE_KILL => {
            require_len(&payload, 1)?;
            Ok(Message::Kill { signal: payload[0] })
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown message type {other}"),
        )),
    }
}

fn require_len(payload: &BytesMut, expected: usize) -> io::Result<()> {
    if payload.len() != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected {expected}-byte payload, got {}", payload.len()),
        ));
    }
    Ok(())
}
