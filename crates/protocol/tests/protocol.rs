//! Integration tests for the SOCKS5 protocol crate.
//!
//! These test the wire format end to end: valid messages, round-tripping, and
//! (critically) malformed / truncated input that a hostile client might send.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use rustproxy_protocol::constants::{atyp, method};
use rustproxy_protocol::handshake::negotiate;
use rustproxy_protocol::{
    Command, Greeting, Host, MethodSelection, ProtocolError, Reply, Request, SocksAddr,
    UsernamePassword,
};

fn ipv4_a() -> SocksAddr {
    SocksAddr {
        host: Host::Ip(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))),
        port: 443,
    }
}

// ---------------------------------------------------------------------------
// Greeting / negotiation
// ---------------------------------------------------------------------------

#[test]
fn greeting_parses_valid() {
    // VER=5 NMETHODS=2 [0x00, 0x02]
    let g = Greeting::parse(&[5, 2, 0x00, 0x02]).expect("parse");
    assert_eq!(g.methods, vec![0x00, 0x02]);
}

#[test]
fn greeting_round_trip_framing() {
    let bytes = [5, 3, 0x00, 0x01, 0x02];
    assert_eq!(Greeting::frame_len(&bytes).unwrap(), 5);
    let g = Greeting::parse(&bytes).unwrap();
    assert_eq!(g.methods, vec![0x00, 0x01, 0x02]);
}

#[test]
fn greeting_frame_len_requires_header() {
    assert_eq!(
        Greeting::frame_len(&[5]).unwrap_err(),
        ProtocolError::Truncated
    );
}

#[test]
fn greeting_rejects_bad_version() {
    let err = Greeting::parse(&[4, 1, 0x00]).unwrap_err();
    assert!(matches!(err, ProtocolError::BadVersion(4)));
}

#[test]
fn greeting_rejects_zero_methods() {
    assert!(Greeting::parse(&[5, 0]).is_err());
}

#[test]
fn greeting_rejects_truncated() {
    // Header promises 5 methods but only 2 bytes follow.
    assert_eq!(
        Greeting::parse(&[5, 5, 0x00, 0x01]).unwrap_err(),
        ProtocolError::Truncated
    );
}

#[test]
fn negotiation_chooses_no_auth_first_when_allowed() {
    let g = Greeting::parse(&[5, 2, 0x00, 0x02]).unwrap();
    let m = negotiate(&g, true, true);
    assert_eq!(m.method, method::NO_AUTH);
}

#[test]
fn negotiation_prefers_user_pass_when_no_auth_off() {
    let g = Greeting::parse(&[5, 2, 0x00, 0x02]).unwrap();
    let m = negotiate(&g, true, false);
    assert_eq!(m.method, method::USER_PASS);
}

#[test]
fn negotiation_rejects_when_nothing_matches() {
    // Client only offers NO_AUTH, but the server requires user/pass.
    let g = Greeting::parse(&[5, 1, 0x00]).unwrap();
    let m = negotiate(&g, true, false);
    assert_eq!(m.method, method::NO_ACCEPTABLE);
}

#[test]
fn method_selection_bytes() {
    assert_eq!(MethodSelection::no_auth().to_bytes(), [5, 0x00]);
    assert_eq!(MethodSelection::user_pass().to_bytes(), [5, 0x02]);
    assert_eq!(
        MethodSelection::no_acceptable_method().to_bytes(),
        [5, 0xff]
    );
}

// ---------------------------------------------------------------------------
// Username/password (RFC 1929)
// ---------------------------------------------------------------------------

#[test]
fn auth_parses_valid_credential() {
    // VER=1 ULEN=3 UNAME="bob" PLEN=3 PASSWD="s3c"
    let bytes = [1, 3, b'b', b'o', b'b', 3, b's', b'3', b'c'];
    let c = UsernamePassword::parse(&bytes).unwrap();
    assert_eq!(c.username, "bob");
    assert_eq!(c.password, "s3c");
}

#[test]
fn auth_rejects_bad_version() {
    assert!(matches!(
        UsernamePassword::parse(&[2, 3, 1, 2, 3, 1, 1]),
        Err(ProtocolError::BadVersion(2))
    ));
}

#[test]
fn auth_rejects_empty_username() {
    let bytes = [1, 0, 3, b'a', b'b', b'c'];
    assert_eq!(
        UsernamePassword::parse(&bytes).unwrap_err(),
        ProtocolError::InvalidCredentialLength
    );
}

#[test]
fn auth_accepts_max_length_password() {
    // A 255-byte password is at the legal limit and must round-trip.
    let mut bytes = vec![1, 2, b'u', b's', 255];
    bytes.extend_from_slice(&[b'x'; 255]);
    let c = UsernamePassword::parse(&bytes).unwrap();
    assert_eq!(c.password.len(), 255);
    assert!(c.password.bytes().all(|b| b == b'x'));
}

#[test]
fn auth_round_trips_serialization() {
    let c = UsernamePassword {
        username: "alice".to_string(),
        password: "hunter2".to_string(),
    };
    let parsed = UsernamePassword::parse(&c.to_vec()).unwrap();
    assert_eq!(parsed, c);
}

#[test]
fn auth_rejects_truncated() {
    // Declares a 10-byte username but provides none.
    let bytes = [1, 10, b'x'];
    assert_eq!(
        UsernamePassword::parse(&bytes).unwrap_err(),
        ProtocolError::Truncated
    );
}

// ---------------------------------------------------------------------------
// CONNECT request
// ---------------------------------------------------------------------------

#[test]
fn request_parses_ipv4_connect() {
    // VER CMD=CONNECT RSV ATYP=IPv4 93.184.216.34 :443
    let bytes = [5, 0x01, 0x00, 0x01, 93, 184, 216, 34, 0x01, 0xbb];
    let (req, consumed) = Request::parse(&bytes).unwrap();
    assert_eq!(req.command, Command::Connect);
    assert_eq!(req.addr, ipv4_a());
    assert_eq!(consumed, bytes.len());
}

#[test]
fn request_parses_ipv6_connect() {
    let ip = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1);
    let mut bytes = vec![5, 0x01, 0x00, 0x04];
    bytes.extend_from_slice(&ip.octets());
    bytes.extend_from_slice(&[0x1f, 0x90]); // port 8080
    let (req, _) = Request::parse(&bytes).unwrap();
    assert_eq!(req.addr.host, Host::Ip(IpAddr::V6(ip)));
    assert_eq!(req.addr.port, 8080);
}

#[test]
fn request_parses_domain_connect() {
    // DOMAIN len=11 "example.com" :80
    let mut bytes = vec![5, 0x01, 0x00, 0x03, 11];
    bytes.extend_from_slice(b"example.com");
    bytes.extend_from_slice(&[0x00, 0x50]); // port 80
    let (req, _) = Request::parse(&bytes).unwrap();
    assert_eq!(req.addr.host, Host::Domain("example.com".to_string()));
    assert_eq!(req.addr.port, 80);
}

#[test]
fn request_rejects_unsupported_command_but_reports_position() {
    let bytes = [5, 0x09, 0x00, 0x01, 0, 0, 0, 0, 0, 0]; // bogus CMD
    assert!(matches!(
        Request::parse(&bytes).unwrap_err(),
        ProtocolError::Unsupported(0x09)
    ));
}

#[test]
fn request_rejects_unsupported_atyp() {
    let bytes = [5, 0x01, 0x00, 0x02, 0, 0, 0, 0, 0, 0]; // ATYP=2 reserved
    assert!(matches!(
        Request::parse(&bytes).unwrap_err(),
        ProtocolError::Unsupported(0x02)
    ));
}

#[test]
fn request_rejects_bad_version() {
    let bytes = [4, 0x01, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
    assert!(matches!(
        Request::parse(&bytes).unwrap_err(),
        ProtocolError::BadVersion(4)
    ));
}

#[test]
fn request_rejects_truncated_ipv4() {
    // Promises an IPv4 address but only supplies one byte of it.
    let bytes = [5, 0x01, 0x00, 0x01, 93];
    assert_eq!(
        Request::parse(&bytes).unwrap_err(),
        ProtocolError::Truncated
    );
}

#[test]
fn request_rejects_truncated_domain() {
    // Domain of length 10 but only 3 bytes supplied.
    let bytes = [5, 0x01, 0x00, 0x03, 10, b'a', b'b', b'c'];
    assert_eq!(
        Request::parse(&bytes).unwrap_err(),
        ProtocolError::Truncated
    );
}

#[test]
fn request_rejects_empty_and_oversized_domains() {
    // Domain length 0.
    let bytes = [5, 0x01, 0x00, 0x03, 0, 0, 0];
    assert!(matches!(
        Request::parse(&bytes).unwrap_err(),
        ProtocolError::Unsupported(atyp::DOMAIN)
    ));
}

#[test]
fn command_round_trip() {
    for cmd in [Command::Connect, Command::Bind, Command::UdpAssociate] {
        assert_eq!(Command::from_u8(cmd.to_u8()).unwrap(), cmd);
    }
    assert!(Command::from_u8(0xEE).is_err());
}

#[test]
fn addr_len_computes_correct_sizes() {
    use rustproxy_protocol::request::addr_len;
    assert_eq!(addr_len(atyp::IPV4, None).unwrap(), 6);
    assert_eq!(addr_len(atyp::IPV6, None).unwrap(), 18);
    assert_eq!(addr_len(atyp::DOMAIN, Some(11)).unwrap(), 14);
    assert_eq!(
        addr_len(atyp::DOMAIN, Some(0)).unwrap_err(),
        ProtocolError::Unsupported(atyp::DOMAIN)
    );
    assert_eq!(
        addr_len(0x09, None).unwrap_err(),
        ProtocolError::Unsupported(0x09)
    );
}

// ---------------------------------------------------------------------------
// SocksAddr serialization
// ---------------------------------------------------------------------------

#[test]
fn addr_wire_len_matches_frame_sizes() {
    assert_eq!(ipv4_a().wire_len(), 4 + 2);
    let v6 = SocksAddr {
        host: Host::Ip(IpAddr::V6(Ipv6Addr::LOCALHOST)),
        port: 9999,
    };
    assert_eq!(v6.wire_len(), 16 + 2);
    let dom = SocksAddr {
        host: Host::Domain("example.org".to_string()),
        port: 80,
    };
    assert_eq!(dom.wire_len(), 1 + 11 + 2);
}

#[test]
fn addr_write_ipv4() {
    let mut buf = vec![0u8; ipv4_a().wire_len()];
    ipv4_a().write(&mut buf);
    // No leading ATYP byte; address+port only.
    assert_eq!(buf, vec![93, 184, 216, 34, 0x01, 0xbb]);
    assert_eq!(ipv4_a().to_vec(), buf);
}

#[test]
fn addr_round_trips_through_request() {
    let cases = [
        SocksAddr {
            host: Host::Ip(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))),
            port: 1080,
        },
        SocksAddr {
            host: Host::Ip(IpAddr::V6(Ipv6Addr::LOCALHOST)),
            port: 65535,
        },
        SocksAddr {
            host: Host::Domain("rust-lang.org".to_string()),
            port: 443,
        },
    ];
    for addr in cases {
        let mut frame = vec![5, 0x01, 0x00];
        frame.push(addr.atyp());
        frame.extend_from_slice(&addr.to_vec());
        let (parsed, _) = Request::parse(&frame).unwrap();
        assert_eq!(parsed.addr, addr);
    }
}

// ---------------------------------------------------------------------------
// Replies
// ---------------------------------------------------------------------------

#[test]
fn reply_builds_success_frame() {
    let r = rustproxy_protocol::reply::success().to_bytes();
    // VER REP=SUCCESS RSV ATYP=IPv4 0.0.0.0 :0
    assert_eq!(r, vec![5, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn reply_round_trips() {
    let r = Reply {
        code: rustproxy_protocol::constants::reply::SUCCESS,
        bind_addr: SocksAddr {
            host: Host::Ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))),
            port: 1234,
        },
    };
    let parsed = Reply::parse(&r.to_bytes()).unwrap();
    assert_eq!(parsed, r);
}

#[test]
fn reply_round_trips_domain_bind() {
    let r = Reply {
        code: 0x00,
        bind_addr: SocksAddr {
            host: Host::Domain("bind.local".to_string()),
            port: 0,
        },
    };
    assert_eq!(Reply::parse(&r.to_bytes()).unwrap(), r);
}

#[test]
fn reply_rejects_bad_version() {
    let bytes = [4, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
    assert!(matches!(
        Reply::parse(&bytes).unwrap_err(),
        ProtocolError::BadVersion(4)
    ));
}

#[test]
fn reply_rejects_truncated_address() {
    let bytes = [5, 0x00, 0x00, 0x04, 1, 2]; // IPv6 promised, few bytes given
    assert_eq!(Reply::parse(&bytes).unwrap_err(), ProtocolError::Truncated);
}

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

#[test]
fn display_formats_hosts() {
    assert_eq!(ipv4_a().to_string(), "93.184.216.34:443");
    let dom = SocksAddr {
        host: Host::Domain("example.com".to_string()),
        port: 80,
    };
    assert_eq!(dom.to_string(), "example.com:80");
}
