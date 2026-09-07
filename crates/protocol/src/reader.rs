//! A small, bounds-checked byte reader used by the parsing functions.
//!
//! It never panics on short input; instead it returns [`ProtocolError::Truncated`]
//! so that a slowly-arriving or malformed client cannot crash the server.

use crate::error::ProtocolError;

/// A cursor over a byte slice that tracks its position and returns
/// [`ProtocolError::Truncated`] rather than panicking on underflow.
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Number of bytes consumed so far.
    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn read_u8(&mut self) -> Result<u8, ProtocolError> {
        let byte = *self.buf.get(self.pos).ok_or(ProtocolError::Truncated)?;
        self.pos += 1;
        Ok(byte)
    }

    pub fn read_u16(&mut self) -> Result<u16, ProtocolError> {
        let hi = self.read_u8()? as u16;
        let lo = self.read_u8()? as u16;
        Ok((hi << 8) | lo)
    }

    /// Read exactly `n` bytes.
    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], ProtocolError> {
        let end = self.pos.checked_add(n).ok_or(ProtocolError::Truncated)?;
        let slice = self
            .buf
            .get(self.pos..end)
            .ok_or(ProtocolError::Truncated)?;
        self.pos = end;
        Ok(slice)
    }
}
