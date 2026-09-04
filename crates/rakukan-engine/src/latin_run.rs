//! 先頭ラテン語ランの復元
//!
//! ひらがなモードのまま英単語を打つと、読みはローマ字がかなに潰れた姿になる。
//! 「seedream」なら `せえdれあm`（トライで解決できなかった子音だけが素の ASCII
//! として残る）。この読みをそのまま [`crate::digits`] のリテラル保護レイヤーへ
//! 渡すと、素の `d` / `m` だけがアルファベット run とみなされ、
//! `せえ` / `れあ` / 後続の日本語が別々に LLM へ渡る。結果は
//! `せえdレアmのぺーす` のような読めない文字列になる（2026-09-01 に実害）。
//!
//! ここでは打鍵ログ（[`crate::Engine::romaji_log_str`]）と読みを突き合わせ、
//! **読みの先頭にあるラテン語ランを打鍵どおりのラテン文字へ戻した読み**を作る。
//! `せえdれあmのぺーすはどう` → `seedreamのぺーすはどう`。
//! これを変換器へ渡せば、既存のリテラル保護レイヤーが
//! `Alpha("seedream")` ＋ `Kana("のぺーすはどう")` に分割し、LLM は日本語部分
//! だけを見る（→ `seedreamのペースはどう`）。
//!
//! # 扱える形が限られる理由
//! 素の ASCII は「ローマ字がかなに潰れきらなかった位置」を示すだけで、英単語の
//! 左右の端そのものは指さない。かなは全てローマ字由来なので、境界の手掛かりは
//! 打鍵ログにも存在しない。誤った位置で切ると日本語側を壊すため、両端が
//! 決まる形だけを扱う:
//!
//! - 左端は「読みの先頭」に限る。途中に出る英単語（`これはseedreamです`）は
//!   どこから始まるか決められない
//! - 右端は「素の ASCII の直後が助詞」の場合に限る。`seedream` は末尾が `m` で
//!   止まるので `のぺーすはどう` との境目が読める。`claude`（読み `cぁうで`）は
//!   素の ASCII が先頭の `c` しか無く右端が決まらないので対象外
//!
//! # 全体がラテン文字の場合は対象外
//! 後続にかなが無い読みは、この関数の目的である「英単語と日本語の境目を
//! 読めるようにする」対象ではないので `None` を返す。

use crate::romaji::RomajiConverter;

/// 英単語の直後に来る助詞。読みの右端を決める唯一の手掛かりに使う。
///
/// 素の ASCII は「ローマ字がかなに潰れきらなかった位置」を示すだけで、英単語が
/// どこで終わるかまでは決めない。`seedream` は末尾が `m` で止まるので右端が
/// 分かるが、`google`（読み `ごおgぇ`）は末尾の `ぇ` まで英単語なのに素の
/// ASCII は途中の `g` が最後になる。そこで切ると `googlれ` に化ける。
///
/// 素の ASCII の直後が助詞なら、そこが英単語の切れ目だと言い切ってよい
/// （`seedream` ＋ `のぺーすはどう`）。助詞でなければ切らない。
fn is_boundary_particle(c: char) -> bool {
    matches!(
        c,
        'の' | 'を' | 'は' | 'が' | 'に' | 'で' | 'と' | 'も' | 'や' | 'へ'
    )
}

/// 打鍵ログを [`crate::Engine::push_char`] と同じ手順で流し直し、
/// 「ローマ字 n 文字まで ↔ かな m 文字まで」の境界一覧を返す。
///
/// 各要素は「未確定バッファを含まない」位置。`seedream` の `m` は次の `n` を
/// 打った時点で初めて素の `m` として確定するが、境界としては `m` まで＝
/// ローマ字 8 文字と記録する。ここで `n` まで含めてしまうと復元結果が
/// `seedreamn` になる。
fn boundaries(romaji: &str) -> Vec<(usize, usize)> {
    let mut conv = RomajiConverter::new();
    let mut pending_len = 0usize;
    let mut kana_len = 0usize;
    let mut marks = Vec::new();
    for (i, c) in romaji.chars().enumerate() {
        pending_len += 1;
        let prev_output_len = conv.output().len();
        let _ = conv.push(c);
        let added_kana = conv.output()[prev_output_len..].chars().count();
        let buffered = conv.buffer().chars().count();
        if pending_len < buffered {
            // 想定外（ログとコンバータがずれた）。以降の対応は信用できない。
            return Vec::new();
        }
        let consumed = pending_len - buffered;
        if consumed > 0 {
            pending_len = buffered;
            kana_len += added_kana;
            marks.push((i + 1 - buffered, kana_len));
        }
    }
    marks
}

/// 打鍵ログ全体をかなへ復元する（末尾の未確定バッファも flush する）。
fn replay(romaji: &str) -> String {
    let mut conv = RomajiConverter::new();
    let mut out = String::new();
    for c in romaji.chars() {
        let prev = conv.output().len();
        let _ = conv.push(c);
        out.push_str(&conv.output()[prev..]);
    }
    out.push_str(&conv.flush());
    out
}

/// 読みの先頭ラテン語ランを打鍵どおりのラテン文字へ戻した読みを返す。
///
/// `romaji` は [`crate::Engine::romaji_log_str`]（未確定バッファを含まない）、
/// `hiragana` は `hiragana_buf` を渡す。復元できない場合は `None`。
pub fn normalize_leading_latin(romaji: &str, hiragana: &str) -> Option<String> {
    if romaji.is_empty() || hiragana.is_empty() {
        return None;
    }
    let kana: Vec<char> = hiragana.chars().collect();
    // 読みに素の ASCII 英字が残っている＝ローマ字がかなに変換しきれていない。
    // 普通の日本語の読みはここで弾かれる。
    let last_ascii = kana.iter().rposition(|c| c.is_ascii_alphabetic())?;
    let split = last_ascii + 1;
    if split >= kana.len() {
        // 後続のかなが無い＝読み全体が英単語。分ける境目が無いので何もしない。
        return None;
    }
    // 素の ASCII の直後が助詞のときだけ「ここで英単語が終わった」と判断する。
    // 助詞以外が続くなら、英単語がまだ続いているのか日本語が始まったのかを
    // 読みから区別できない（`ごおgぇ` = google / `せえdれあmつかう` = 英単語＋助詞なしの続き）。
    if !is_boundary_particle(kana[split]) {
        return None;
    }
    // F9/F10 の force_preedit 後などログと読みが対応しないケースを弾く。
    if replay(romaji) != hiragana {
        return None;
    }
    let romaji_chars: Vec<char> = romaji.chars().collect();
    let romaji_prefix_len = boundaries(romaji)
        .into_iter()
        .find(|&(_, kana_len)| kana_len == split)
        .map(|(romaji_len, _)| romaji_len)?;
    let head: String = romaji_chars.get(..romaji_prefix_len)?.iter().collect();
    // 記号・数字が混ざるものは digits.rs のリテラル保護レイヤーの担当。
    if !head.chars().all(|c| c.is_ascii_alphanumeric())
        || !head.chars().any(|c| c.is_ascii_alphabetic())
    {
        return None;
    }
    let tail: String = kana[split..].iter().collect();
    Some(format!("{head}{tail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_leading_latin_word_before_kana() {
        // 「seedreamのぺーすはどう」と打った状態
        assert_eq!(
            normalize_leading_latin("seedreamnope-suhadou", "せえdれあmのぺーすはどう").as_deref(),
            Some("seedreamのぺーすはどう")
        );
    }

    #[test]
    fn restores_leading_latin_word_mid_typing() {
        // 変換が追いつく前（`のぺー` まで打った時点）でも同じ位置で切れる
        assert_eq!(
            normalize_leading_latin("seedreamnope-", "せえdれあmのぺー").as_deref(),
            Some("seedreamのぺー")
        );
    }

    #[test]
    fn ignores_pure_kana_reading() {
        assert_eq!(normalize_leading_latin("kanntanna", "かんたんな"), None);
    }

    #[test]
    fn ignores_reading_that_is_entirely_latin() {
        // 読み全体が英単語 → 分ける境目が無いので対象外
        assert_eq!(normalize_leading_latin("seedream", "せえdれあm"), None);
    }

    /// `google` / `ありがとう` の読みが想定どおりかを先に固定する。
    /// ここがずれていると下の 2 テストが「復元しない」理由を取り違える。
    #[test]
    fn replay_matches_expected_readings() {
        assert_eq!(replay("google"), "ごおgぇ");
        assert_eq!(replay("seedreamtsukau"), "せえdれあmつかう");
        assert_eq!(replay("seedreamnope-suhadou"), "せえdれあmのぺーすはどう");
    }

    #[test]
    fn ignores_latin_word_whose_right_edge_is_unknown() {
        // google は末尾の `ぇ` まで英単語だが、素の ASCII は途中の `g` が最後。
        // ここで切ると `googlれ` に化けるので切らない。
        assert_eq!(normalize_leading_latin("google", "ごおgぇ"), None);
    }

    #[test]
    fn ignores_latin_word_not_followed_by_a_particle() {
        // 助詞以外が続くと、英単語が終わったのか続いているのか読みから決まらない
        assert_eq!(
            normalize_leading_latin("seedreamtsukau", "せえdれあmつかう"),
            None
        );
    }

    #[test]
    fn ignores_reading_out_of_sync_with_log() {
        // force_preedit 後を模した不一致
        assert_eq!(normalize_leading_latin("seedreamno", "SEEDREAMの"), None);
    }
}
