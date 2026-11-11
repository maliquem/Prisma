use serde::{Serialize, de::DeserializeOwned};
use std::io::{self, ErrorKind, Read, Write};

use crate::MAX_FRAME;

pub fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
    let len = u32::try_from(body.len())
        .ok()
        .filter(|n| *n <= MAX_FRAME)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "quadro grande demais"))?;
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&body);
    w.write_all(&out)?;
    w.flush()
}

/// `Ok(None)` = fim limpo antes do prefixo.
pub fn read_frame<R: Read, T: DeserializeOwned>(r: &mut R) -> io::Result<Option<T>> {
    let mut prefix = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match r.read(&mut prefix[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(ErrorKind::UnexpectedEof.into()),
            Ok(n) => got += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let len = u32::from_le_bytes(prefix);
    if len > MAX_FRAME {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "quadro grande demais",
        ));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| io::Error::new(ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Page, Reply, Request};
    use std::io::{Cursor, ErrorKind};

    fn rt<T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(v: T) {
        let mut buf = Vec::new();
        write_frame(&mut buf, &v).unwrap();
        let back: Option<T> = read_frame(&mut Cursor::new(buf)).unwrap();
        assert_eq!(back, Some(v));
    }

    #[test]
    fn every_request_round_trips() {
        rt(Request::Hello {
            protocol: 1,
            app_version: "0.1.0".into(),
        });
        rt(Request::GameStarted {
            pid: 9,
            exe: r"C:\g.exe".into(),
            name: "G".into(),
        });
        rt(Request::GameStopped { pid: 9 });
        for page in [Page::Meter, Page::Share, Page::GameProfile] {
            rt(Request::OpenPage { page });
        }
        rt(Request::ShareInvite { minutes: None });
        rt(Request::SaveReplay);
    }

    #[test]
    fn every_reply_round_trips() {
        rt(Reply::Capabilities {
            protocol: 1,
            app_version: "1".into(),
            caps: vec!["a".into()],
        });
        rt(Reply::ShareInviteCode {
            code: "abc".into(),
            link: "prisma://x".into(),
        });
        rt(Reply::ShareInviteFailed {
            reason: "no".into(),
        });
    }

    #[test]
    fn prefix_is_u32_le() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &Request::ShareInvite { minutes: None }).unwrap();
        let json = serde_json::to_string(&Request::ShareInvite { minutes: None }).unwrap();
        assert_eq!(&buf[..4], &(json.len() as u32).to_le_bytes());
        assert_eq!(&buf[4..], json.as_bytes());
    }

    #[test]
    fn clean_eof_is_none() {
        let r: Option<Request> = read_frame(&mut Cursor::new(Vec::<u8>::new())).unwrap();
        assert_eq!(r, None);
    }

    #[test]
    fn oversized_frame_is_rejected_without_reading_body() {
        let buf = 65_537u32.to_le_bytes().to_vec();
        let e = read_frame::<_, Request>(&mut Cursor::new(buf)).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn bad_json_is_invalid_data() {
        let mut buf = 3u32.to_le_bytes().to_vec();
        buf.extend_from_slice(b"{{{");
        let e = read_frame::<_, Request>(&mut Cursor::new(buf)).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn truncated_body_is_error() {
        let mut buf = 10u32.to_le_bytes().to_vec();
        buf.extend_from_slice(b"abcd");
        let e = read_frame::<_, Request>(&mut Cursor::new(buf)).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::UnexpectedEof);
    }
}
