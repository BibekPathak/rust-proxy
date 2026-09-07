//! SOCKS5 username/password authentication (RFC 1929).

use crate::constants::AUTH_VERSION;
use crate::error::ProtocolError;
use crate::reader::Reader;

/// RFC 1929 authentication status codes.
pub mod auth_status {
    /// Credentials accepted.
    pub const SUCCESS: u8 = 0x00;
    /// Credentials rejected (or verification failed).
    pub const FAILURE: u8 = 0x01;
}

/// Maximum length (in bytes) of a username or password per RFC 1929 §2.
pub const MAX_USERNAME_LEN: usize = 255;
pub const MAX_PASSWORD_LEN: usize = 255;

/// A parsed username/password credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsernamePassword {
    pub username: String,
    pub password: String,
}

impl UsernamePassword {
    /// Parse a full RFC 1929 credential frame.
    ///
    /// Layout: `VER(1) ULEN(1) UNAME(ULEN) PLEN(1) PASSWD(PLEN)`.
    pub fn parse(buf: &[u8]) -> Result<UsernamePassword, ProtocolError> {
        let mut r = Reader::new(buf);
        let ver = r.read_u8()?;
        if ver != AUTH_VERSION {
            return Err(ProtocolError::BadVersion(ver));
        }
        let ulen = r.read_u8()? as usize;
        if ulen == 0 || ulen > MAX_USERNAME_LEN {
            return Err(ProtocolError::InvalidCredentialLength);
        }
        let username_bytes = r.read_bytes(ulen)?;
        let plen = r.read_u8()? as usize;
        if plen > MAX_PASSWORD_LEN {
            return Err(ProtocolError::InvalidCredentialLength);
        }
        let password_bytes = r.read_bytes(plen)?;

        let username = String::from_utf8(username_bytes.to_vec())
            .map_err(|_| ProtocolError::InvalidCredentialLength)?;
        let password = String::from_utf8(password_bytes.to_vec())
            .map_err(|_| ProtocolError::InvalidCredentialLength)?;
        Ok(UsernamePassword { username, password })
    }

    /// Total wire length of this credential frame.
    pub fn wire_len(&self) -> usize {
        1 + 1 + self.username.len() + 1 + self.password.len()
    }

    /// Serialize the credential frame into a freshly allocated buffer.
    pub fn to_vec(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.wire_len());
        buf.push(AUTH_VERSION);
        buf.push(self.username.len() as u8);
        buf.extend_from_slice(self.username.as_bytes());
        buf.push(self.password.len() as u8);
        buf.extend_from_slice(self.password.as_bytes());
        buf
    }
}

/// The server's 2-byte authentication verdict frame: `VER STATUS`.
pub fn auth_reply_bytes(status: u8) -> [u8; 2] {
    [AUTH_VERSION, status]
}
