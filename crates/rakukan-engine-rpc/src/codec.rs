//! length-prefixed postcard フレーミング。
//!
//! ワイヤフォーマット: `[u32 little-endian length][payload bytes]`
//!
//! `std::io::Read` / `std::io::Write` に対する薄いユーティリティ。
//! Named Pipe は同期 I/O で使うため async は不要。

use std::io::{Read, Write};

use anyhow::{Context, Result, bail};
use serde::{Serialize, de::DeserializeOwned};

/// 1 フレームあたりの最大バイト数（DoS 対策）。
/// llama 出力の候補 JSON でも通常数 KB なので 8 MiB で十分。
const MAX_FRAME_BYTES: u32 = 8 * 1024 * 1024;

pub fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> Result<()> {
    let payload = postcard::to_allocvec(msg).context("postcard encode")?;
    if payload.len() as u64 > MAX_FRAME_BYTES as u64 {
        bail!("frame too large: {} bytes", payload.len());
    }
    let len = payload.len() as u32;
    w.write_all(&len.to_le_bytes()).context("write length")?;
    w.write_all(&payload).context("write payload")?;
    w.flush().context("flush frame")?;
    Ok(())
}

pub fn read_frame<R: Read, T: DeserializeOwned>(r: &mut R) -> Result<T> {
    let mut len_bytes = [0u8; 4];
    r.read_exact(&mut len_bytes).context("read length")?;
    let len = u32::from_le_bytes(len_bytes);
    if len > MAX_FRAME_BYTES {
        bail!("frame too large: {} bytes", len);
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).context("read payload")?;
    postcard::from_bytes(&buf).context("postcard decode")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Request, Response};
    use std::io::Cursor;

    #[test]
    fn roundtrip_request() {
        let mut buf = Vec::new();
        let req = Request::PushChar('あ' as u32);
        write_frame(&mut buf, &req).unwrap();
        let mut cur = Cursor::new(&buf);
        let got: Request = read_frame(&mut cur).unwrap();
        assert!(matches!(got, Request::PushChar(x) if x == 'あ' as u32));
    }

    #[test]
    fn roundtrip_response_strings() {
        let mut buf = Vec::new();
        let resp = Response::Strings(vec!["漢字".into(), "感じ".into()]);
        write_frame(&mut buf, &resp).unwrap();
        let mut cur = Cursor::new(&buf);
        let got: Response = read_frame(&mut cur).unwrap();
        match got {
            Response::Strings(v) => assert_eq!(v, vec!["漢字", "感じ"]),
            _ => panic!("wrong variant"),
        }
    }
}
