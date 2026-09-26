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
    use crate::protocol::{
        ChangeKind, ChangeOutcome, ChangeRequest, EngineGen, Expect, HostId, InputCharKind, Owner,
        Reason, Request, Response, TsfId, Unresolved,
    };
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

    #[test]
    fn roundtrip_shutdown_if_config_differs() {
        let mut buf = Vec::new();
        let req = Request::ShutdownIfConfigDiffers {
            config_json: Some(r#"{"num_candidates":9}"#.into()),
            expected_host_id: HostId(7),
            config_version: Some([3; 32]),
        };
        write_frame(&mut buf, &req).unwrap();
        let mut cur = Cursor::new(&buf);
        let got: Request = read_frame(&mut cur).unwrap();
        assert!(matches!(
            got,
            Request::ShutdownIfConfigDiffers {
                config_json: Some(s),
                expected_host_id: HostId(7),
                config_version: Some(v),
            } if s == r#"{"num_candidates":9}"# && v == [3; 32]
        ));
    }

    /// 新 variant は enum 末尾に追加する規約の検証: postcard は宣言順 discriminant
    /// なので、途中挿入すると既存 variant のワイヤ表現が壊れる。Shutdown が
    /// 「1 バイトの varint discriminant のみ（ペイロードなし）」であることを固定し、
    /// 誤ってペイロードを足す変更を検出する。
    #[test]
    fn existing_variant_wire_format_is_stable() {
        let mut buf = Vec::new();
        write_frame(
            &mut buf,
            &Request::Shutdown {
                expected_host_id: HostId(0),
            },
        )
        .unwrap();
        // [len=4bytes LE][varint discriminant][u128 host id]
        let payload = &buf[4..];
        assert_eq!(payload[0], 50, "Shutdown discriminant must stay stable");
    }

    #[test]
    fn roundtrip_issue56_protocol_shapes() {
        let owner = Owner {
            tsf_id: TsfId(11),
            composition: 12,
        };
        let engine_gen = EngineGen {
            host_id: HostId(13),
            generation: 14,
        };
        let expect = Expect { engine_gen, owner };
        let unresolved = Unresolved {
            seq: 15,
            kind: ChangeKind::InputChar,
            owner,
            engine_gen,
        };
        let req = Request::Restore {
            owner,
            seq: unresolved.seq,
            reading: "た".into(),
            pending_romaji: "t".into(),
            then: Some(ChangeRequest::InputChar {
                c: 'a' as u32,
                kind: InputCharKind::Char,
                bg_start_n_cands: Some(6),
            }),
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &req).unwrap();
        let got: Request = read_frame(&mut Cursor::new(&buf)).unwrap();
        assert!(
            matches!(got, Request::Restore { owner: got_owner, seq: 15, .. } if got_owner == owner)
        );

        let change = Request::Change {
            seq: 16,
            expect,
            request: ChangeRequest::Backspace,
            config_version: None,
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &change).unwrap();
        let got: Request = read_frame(&mut Cursor::new(&buf)).unwrap();
        assert!(matches!(
            got,
            Request::Change {
                seq: 16,
                request: ChangeRequest::Backspace,
                config_version: None,
                ..
            }
        ));

        let responses = [
            Response::Rejected(Reason::OwnerMismatch),
            Response::Restored {
                engine_gen,
                then: Some(ChangeOutcome::InputChar {
                    preedit: "たa".into(),
                    hiragana: "た".into(),
                    bg_status: "idle".into(),
                }),
            },
            Response::Changed {
                outcome: ChangeOutcome::Candidates(vec!["多".into()]),
            },
        ];
        for response in responses {
            let mut buf = Vec::new();
            write_frame(&mut buf, &response).unwrap();
            let _: Response = read_frame(&mut Cursor::new(&buf)).unwrap();
        }

        assert_eq!(expect.engine_gen, unresolved.engine_gen);
    }

    fn discriminant(req: &Request) -> u8 {
        let mut buf = Vec::new();
        write_frame(&mut buf, req).unwrap();
        buf[4]
    }

    /// 廃止した `MergeCandidates` は `_ReservedMergeCandidates` としてスロットを残す。
    /// 削除してしまうと後続 variant（`MergeCandidatesForReading` 等）の discriminant が
    /// ずれて、古い host / 新しい TSF の組み合わせで別の要求として解釈される。
    #[test]
    #[allow(deprecated)]
    fn removed_merge_candidates_keeps_its_slot() {
        let reserved = discriminant(&Request::_ReservedMergeCandidates {
            llm_cands: vec![],
            limit: 0,
        });
        // ConvertSync, _ReservedConvertSyncSegmented, _ReservedMergeCandidates の並び
        assert_eq!(reserved, discriminant(&Request::ConvertSync) + 2);
    }
}
