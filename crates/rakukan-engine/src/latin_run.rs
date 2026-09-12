//! 先頭ラテン語ランの復元
//!
//! ひらがなモードのまま英単語を打つと、読みはローマ字が部分的にかなへ潰れた姿に
//! なる。`seedream` なら `せえdれあm`（トライで解決できなかった子音だけが素の
//! ASCII として残る）。この読みをそのまま [`crate::digits`] のリテラル保護レイヤーへ
//! 渡すと、素の `d` / `m` だけがアルファベット run とみなされ、`せえ` / `れあ` /
//! 後続の日本語が別々に LLM へ渡る。結果は `せえdレアmのぺーす` のような読めない
//! 文字列になる。
//!
//! ここでは打鍵ログ（[`crate::InputEntry`]）と読みを突き合わせ、**読みの先頭にある
//! ラテン語ランを打鍵どおりのラテン文字へ戻した読み**を作る。
//! `せえdれあmのぺーすはどう` → `seedreamのぺーすはどう`。これを変換器へ渡せば、
//! 既存のリテラル保護レイヤーが `Alpha("seedream")` ＋ `Kana("のぺーすはどう")` に
//! 分割し、LLM は日本語部分だけを見る（→ `seedreamのペースはどう`）。
//!
//! # 境界をログから読む
//! 各エントリは「打鍵（`typed`）」と「そのとき `hiragana_buf` に足した文字列
//! （`output`）」を持ち、未確定ローマ字（`pending_romaji_buf`）はログに入らない。
//! したがって `output` を累積した位置が、そのままローマ字とかなの対応表になる。
//! 変換器に流し直して対応を作り直す必要はない。
//!
//! # 扱える形が限られる理由
//! 素の ASCII は「ローマ字がかなに潰れきらなかった位置」を示すだけで、英単語の
//! 左右の端そのものは指さない。かなは全てローマ字由来なので、境界の手掛かりは
//! 打鍵ログにも存在しない。誤った位置で切ると日本語側を壊すため、両端が決まる形
//! だけを扱う:
//!
//! - 右端は「素の ASCII の直後が助詞」の場合に限る。`seedream` は末尾が `m` で
//!   止まるので `のぺーすはどう` との境目が読める。`google`（読み `ごおgぇ`）は
//!   素の ASCII が途中の `g` で終わるため右端が決まらず、対象外
//! - 左端は「読みの先頭」に限る。しかも先頭であること自体は証明できないので、
//!   ラテン語ランの範囲に助詞のかなが現れたら日本語の前置きを疑って手を引く
//!   （`これはseedreamの…` の `これは` を巻き込まないため）
//!
//! # 既知の限界: 助詞を含まない日本語の前置き
//! 左端の判定は「ラテン語ランの範囲に助詞が無い」ことしか見ないので、助詞を含まない
//! 日本語の前置き（`きょう` `いま` `あした` など）に英単語が続く読みは前置きごと
//! ラテン語ランとみなされる（`kyouseedreamnope-suhadou` → `kyouseedreamのぺーすはどう`）。
//! 前置きが日本語か英単語の一部かは読みからもログからも決まらない。誤発動するのは
//! 読みに素の ASCII が残っている（＝英単語をひらがなモードで打った時点で既に壊れて
//! いる）場合に限られ、正常な日本語の変換を壊すことはないので、この方式の限界として
//! 受け入れている（PR #44 のレビュー、2026-09-12）。
//!
//! # 全体がラテン文字の場合は対象外
//! 後続にかなが無い読みは、この関数の目的である「英単語と日本語の境目を読める
//! ようにする」対象ではないので `None` を返す。

/// 英単語の直後に来る助詞。読みの右端を決める唯一の手掛かりであり、
/// 左端に日本語の前置きが無いことを疑うための手掛かりでもある。
const PARTICLES: [char; 10] = ['の', 'を', 'は', 'が', 'に', 'で', 'と', 'も', 'や', 'へ'];

fn is_boundary_particle(c: char) -> bool {
    PARTICLES.contains(&c)
}

/// 読みの先頭ラテン語ランを打鍵どおりのラテン文字へ戻した読みを返す。
///
/// `log` は [`crate::RakunEngine`] の `input_log`、`detached_at` は
/// `log_detached_at`、`hiragana` は `hiragana_buf` を渡す。復元できない場合は
/// `None`（呼び出し側は読みをそのまま使う）。
pub(crate) fn normalize_leading_latin(
    log: &[crate::InputEntry],
    detached_at: usize,
    hiragana: &str,
) -> Option<String> {
    // F9/F10 の force_preedit 後はログと読みが対応しない。
    if hiragana.is_empty() || log.is_empty() || detached_at != 0 {
        return None;
    }
    // Backspace 再生などでログと読みがずれていたら手を出さない。
    let logged: String = log.iter().map(|entry| entry.output.as_str()).collect();
    if logged != hiragana {
        return None;
    }

    let chars: Vec<char> = hiragana.chars().collect();
    // 読みに素の ASCII 英字が残っている＝ローマ字がかなに変換しきれていない。
    // 普通の日本語の読みはここで弾かれる。
    let split = chars.iter().rposition(|c| c.is_ascii_alphabetic())? + 1;
    if split >= chars.len() {
        // 後続のかなが無い＝読み全体が英単語。分ける境目が無いので何もしない。
        return None;
    }
    // 素の ASCII の直後が助詞のときだけ「ここで英単語が終わった」と判断する。
    // 助詞以外が続くなら、英単語がまだ続いているのか日本語が始まったのかを読みから
    // 区別できない（`ごおgぇ` = google / `せえdれあmつかう` = 英単語＋助詞なしの続き）。
    if !is_boundary_particle(chars[split]) {
        return None;
    }
    // ラテン語ランの範囲に助詞が混ざっていたら、そこまでが日本語の前置きである
    // 可能性を否定できないので復元しない。`これはせえdれあmのぺーす` は最後の素
    // ASCII が `m`・直後が `の` なので上の判定は通ってしまうが、ここで `は` を見て
    // 手を引く。前置きを巻き込むと `korehaseedream` がリテラル化され、日本語側が
    // 丸ごと壊れる。英単語のローマ字が助詞と同じかなを生む語（`monitor` →
    // `もにとr`）も復元されなくなるが、その場合は復元しないだけで害が無い。
    if chars[..split].iter().copied().any(is_boundary_particle) {
        return None;
    }

    // `output` の累積がちょうど `split` になるエントリ境界を探す。境界がエントリの
    // 内側にある（跨いでしまう）場合は復元できない。
    let mut output_len = 0;
    let end = log.iter().position(|entry| {
        output_len += entry.output.chars().count();
        output_len == split
    })? + 1;
    // 記号・数字・Shift+英字が混ざる範囲は digits.rs のリテラル保護レイヤーの担当。
    if log[..end]
        .iter()
        .any(|entry| entry.kind != crate::InputKind::Romaji)
    {
        return None;
    }
    let mut head: String = log[..end]
        .iter()
        .map(|entry| entry.typed.as_str())
        .collect();
    if !head.chars().all(|c| c.is_ascii_alphanumeric())
        || !head.chars().any(|c| c.is_ascii_alphabetic())
    {
        return None;
    }
    head.extend(chars[split..].iter());
    Some(head)
}

#[cfg(test)]
mod tests {
    fn engine_after(typed: &str) -> crate::RakunEngine {
        let mut e = crate::RakunEngine::new(crate::EngineConfig::default());
        for c in typed.chars() {
            e.push_char(c);
        }
        e
    }

    fn conv_reading_after(typed: &str) -> String {
        engine_after(typed).conv_reading()
    }

    /// 各ケースの読みが想定どおりかを先に固定する。ここがずれていると、下の
    /// テストが「復元する / しない」の理由を取り違える。
    /// `seedream` 単体の末尾 `m` は次の打鍵まで未確定バッファに残るので読みに出ない。
    #[test]
    fn readings_match_expected() {
        let cases = [
            ("seedreamnope-suhadou", "せえdれあmのぺーすはどう"),
            ("seedreamnope-", "せえdれあmのぺー"),
            ("kanntannna", "かんたんな"),
            ("seedream", "せえdれあ"),
            ("google", "ごおgぇ"),
            ("seedreamtsukau", "せえdれあmつかう"),
            (
                "korehaseedreamnope-suhadou",
                "これはせえdれあmのぺーすはどう",
            ),
        ];
        for (typed, reading) in cases {
            assert_eq!(
                engine_after(typed).hiragana_text(),
                reading,
                "typed={typed}"
            );
        }
    }

    #[test]
    fn restores_leading_latin_word_before_kana() {
        // 「seedreamのぺーすはどう」と打った状態
        assert_eq!(
            conv_reading_after("seedreamnope-suhadou"),
            "seedreamのぺーすはどう"
        );
    }

    #[test]
    fn restores_leading_latin_word_mid_typing() {
        // 変換が追いつく前（`のぺー` まで打った時点）でも同じ位置で切れる
        assert_eq!(conv_reading_after("seedreamnope-"), "seedreamのぺー");
    }

    #[test]
    fn ignores_pure_kana_reading() {
        assert_eq!(conv_reading_after("kanntannna"), "かんたんな");
    }

    #[test]
    fn ignores_reading_that_is_entirely_latin() {
        // 読み全体が英単語 → 分ける境目が無いので対象外
        assert_eq!(conv_reading_after("seedream"), "せえdれあ");
    }

    #[test]
    fn ignores_latin_word_whose_right_edge_is_unknown() {
        // google は末尾の `ぇ` まで英単語だが、素の ASCII は途中の `g` が最後。
        // ここで切ると `googlれ` に化けるので切らない。
        assert_eq!(conv_reading_after("google"), "ごおgぇ");
    }

    #[test]
    fn ignores_latin_word_not_followed_by_a_particle() {
        // 助詞以外が続くと、英単語が終わったのか続いているのか読みから決まらない
        assert_eq!(conv_reading_after("seedreamtsukau"), "せえdれあmつかう");
    }

    #[test]
    fn ignores_latin_run_after_japanese_prefix() {
        // 前置き `これは` は日本語だが、読みの上では英単語の一部と区別できない。
        // 巻き込んで `korehaseedream` をリテラル化すると日本語側が壊れる。
        assert_eq!(
            conv_reading_after("korehaseedreamnope-suhadou"),
            "これはせえdれあmのぺーすはどう"
        );
    }

    #[test]
    fn folds_particle_free_japanese_prefix_known_limitation() {
        // 既知の限界（モジュール doc 参照）: 助詞を含まない前置き `きょう` / `いま` は
        // 英単語の一部と区別できず、ラテン語ランに巻き込まれる。挙動を固定しておき、
        // 判定を変えたときに気づけるようにする。
        assert_eq!(
            conv_reading_after("kyouseedreamnope-suhadou"),
            "kyouseedreamのぺーすはどう"
        );
        assert_eq!(
            conv_reading_after("imaseedreamnope-suhadou"),
            "imaseedreamのぺーすはどう"
        );
    }

    #[test]
    fn ignores_reading_out_of_sync_with_log() {
        // F9/F10 の force_preedit 後はログと読みが対応しない
        let mut e = engine_after("seedreamno");
        e.force_preedit("SEEDREAMの".to_string());
        assert_ne!(e.log_detached_at, 0);
        assert_eq!(e.conv_reading(), "SEEDREAMの");
    }
}
