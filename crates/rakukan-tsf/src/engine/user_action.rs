//! キー入力 → UserAction 変換

#[derive(Debug, Clone, PartialEq)]
pub enum UserAction {
    // ─── 文字入力 ────────────────────────────────────────────────────────
    Input(char),
    /// 全角スペース（Shift+Space）
    FullWidthSpace,

    // ─── プリエディット操作 ──────────────────────────────────────────────
    /// 変換開始（Space / 変換キー）
    Convert,
    /// ひらがなのまま確定（Enter）
    CommitRaw,
    /// 1文字削除（変換中なら変換取り消し）
    Backspace,
    /// プリエディット全破棄（Ctrl+Backspace）
    CancelAll,
    /// Escape: 変換中→変換取り消し / 未変換→プリエディット破棄
    Cancel,

    // ─── 文字種変換（プリエディット確定前）──────────────────────────────
    Hiragana,           // F6
    Katakana,           // F7
    HalfKatakana,       // F8
    FullLatin,          // F9
    HalfLatin,          // F10
    /// 無変換: ひらがな→カタカナ→半角カタカナ 循環
    CycleKana,

    // ─── 候補ウィンドウ操作 ──────────────────────────────────────────────
    CandidateNext,       // Tab / ↓ / Space（変換中）
    CandidatePrev,       // Shift+Tab / ↑
    CandidatePageDown,   // PageDown
    CandidatePageUp,     // PageUp
    CandidateSelect(u8), // 数字 1–9

    // ─── IME オン/オフ ───────────────────────────────────────────────────
    /// IME をオフにする（英数パススルーモードへ）
    ImeOff,
    /// IME をオンにする（直前のかなモードへ戻る）
    ImeOn,
    /// IME オン/オフ トグル（全角/半角キー）
    ImeToggle,

    // ─── 入力モード切り替え（IME オン中）────────────────────────────────
    /// ひらがな入力モードへ
    ModeHiragana,
    /// カタカナ入力モードへ（全角）
    ModeKatakana,
    /// 半角英数入力モードへ
    ModeAlphanumeric,

    // ─── カーソル移動（プリエディット内）────────────────────────────────
    CursorLeft,
    CursorRight,

    Tab,
    Unknown,
}
