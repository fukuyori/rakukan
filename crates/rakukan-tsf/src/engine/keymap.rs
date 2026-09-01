//! キーバインド設定（MS-IME 準拠デフォルト）
//!
//! 設定ファイル: `%APPDATA%\rakukan\keymap.toml`
//! リロードタイミング:
//! - Activate（IME オフ → オン）: 必ず読み込む（`Keymap::load`）
//! - 入力モード切替: `keymap.toml` の mtime が変わった場合だけ読み直す
//!   （`Keymap::reload_if_changed`）。以前は切替ごとに同期でファイル読込 + TOML parse を
//!   行っており、8月ログで SLOW OnKeyDown の直前に `keymap loaded` が 53 回、最悪 931ms。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::user_action::UserAction;

// ─── KeyAction ───────────────────────────────────────────────────────────────

/// 設定ファイルに書けるアクション名
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    Convert,   // Space, 変換キー
    CommitRaw, // Enter（ひらがなのまま確定）
    Backspace,
    CancelAll,         // Ctrl+Backspace（プリエディット全破棄）
    Cancel,            // Escape
    Hiragana,          // F6
    Katakana,          // F7
    HalfKatakana,      // F8
    FullLatin,         // F9
    HalfLatin,         // F10
    CycleKana,         // 無変換
    FullWidthSpace,    // Shift+Space
    CandidateNext,     // Tab, ↓
    CandidatePrev,     // Shift+Tab, ↑
    CandidatePageDown, // PageDown
    CandidatePageUp,   // PageUp
    CandidateN(u8),    // 数字 1–9
    // IME オン/オフ
    ImeOff,    // 英数キー（IME オン中）
    ImeOn,     // 英数キー以外（IME オフ中）
    ImeToggle, // 全角/半角, Ctrl+Space
    // 入力モード切り替え（IME オン中）
    ModeHiragana,     // ひらがなキー, Ctrl+Caps
    ModeKatakana,     // カタカナキー, Alt+Caps
    ModeAlphanumeric, // 英数キー
    CursorLeft,
    CursorRight,
    /// Home（Issue #11: preedit 中はアプリへ渡さない）
    CursorHome,
    /// End（同上）
    CursorEnd,
    /// 文節縮小（Shift+Left）
    SegmentShrink,
    /// 文節拡大（Shift+Right）
    SegmentExtend,
}

impl KeyAction {
    pub fn to_user_action(&self) -> UserAction {
        match self {
            Self::Convert => UserAction::Convert,
            Self::CommitRaw => UserAction::CommitRaw,
            Self::Backspace => UserAction::Backspace,
            Self::CancelAll => UserAction::CancelAll,
            Self::Cancel => UserAction::Cancel,
            Self::Hiragana => UserAction::Hiragana,
            Self::Katakana => UserAction::Katakana,
            Self::HalfKatakana => UserAction::HalfKatakana,
            Self::FullLatin => UserAction::FullLatin,
            Self::HalfLatin => UserAction::HalfLatin,
            Self::CycleKana => UserAction::CycleKana,
            Self::FullWidthSpace => UserAction::FullWidthSpace,
            Self::CandidateNext => UserAction::CandidateNext,
            Self::CandidatePrev => UserAction::CandidatePrev,
            Self::CandidatePageDown => UserAction::CandidatePageDown,
            Self::CandidatePageUp => UserAction::CandidatePageUp,
            Self::CandidateN(n) => UserAction::CandidateSelect(*n),
            Self::ImeOff => UserAction::ImeOff,
            Self::ImeOn => UserAction::ImeOn,
            Self::ImeToggle => UserAction::ImeToggle,
            Self::ModeHiragana => UserAction::ModeHiragana,
            Self::ModeKatakana => UserAction::ModeKatakana,
            Self::ModeAlphanumeric => UserAction::ModeAlphanumeric,
            Self::CursorLeft => UserAction::CursorLeft,
            Self::CursorRight => UserAction::CursorRight,
            Self::CursorHome => UserAction::CursorHome,
            Self::CursorEnd => UserAction::CursorEnd,
            Self::SegmentShrink => UserAction::SegmentShrink,
            Self::SegmentExtend => UserAction::SegmentExtend,
        }
    }
}

// ─── KeySpec ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeySpec {
    pub vk: u16,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl KeySpec {
    pub fn parse(s: &str) -> Option<Self> {
        let mut ctrl = false;
        let mut shift = false;
        let mut alt = false;
        let mut vk: Option<u16> = None;
        for part in s.split('+') {
            match part.trim().to_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "shift" => shift = true,
                "alt" => alt = true,
                name => vk = Some(name_to_vk(name)?),
            }
        }
        Some(Self {
            vk: vk?,
            ctrl,
            shift,
            alt,
        })
    }
}

fn name_to_vk(name: &str) -> Option<u16> {
    Some(match name {
        "backspace" | "bs" => 0x08,
        "tab" => 0x09,
        "enter" | "return" => 0x0D,
        "escape" | "esc" => 0x1B,
        "space" => 0x20,
        "backquote" | "grave" => 0xC0,
        "semicolon" => 0xBA,
        "equal" => 0xBB,
        "comma" => 0xBC,
        "minus" => 0xBD,
        "period" => 0xBE,
        "slash" => 0xBF,
        "leftbracket" => 0xDB,
        "backslash" => 0xDC,
        "rightbracket" => 0xDD,
        "quote" => 0xDE,
        "pageup" | "pgup" => 0x21,
        "pagedown" | "pgdn" => 0x22,
        "end" => 0x23,
        "home" => 0x24,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "delete" | "del" => 0x2E,
        "f1" => 0x70,
        "f2" => 0x71,
        "f3" => 0x72,
        "f4" => 0x73,
        "f5" => 0x74,
        "f6" => 0x75,
        "f7" => 0x76,
        "f8" => 0x77,
        "f9" => 0x78,
        "f10" => 0x79,
        "f11" => 0x7A,
        "f12" => 0x7B,
        // 全角/半角キー。JIS 配列の実際の VK は 0xF3 / 0xF4 だが、
        // normalize_key_event_vk() が 0x19 に寄せてから resolve_action へ渡す。
        "zenkaku" | "hankaku" | "kanji" => 0x19, // VK_KANJI
        "henkan" => 0x1C,
        "muhenkan" => 0x1D,
        "eisuu" | "alphanumeric" => 0xF0, // 英数キー
        "katakana" => 0xF1,               // カタカナキー
        "hiragana_key" => 0xF2,           // ひらがなキー
        "caps" => 0x14,                   // Caps Lock
        // 単一アルファベット (a-z → VK 0x41-0x5A)
        name if name.len() == 1 => {
            let c = name.chars().next().unwrap();
            if c.is_ascii_alphabetic() {
                c.to_ascii_uppercase() as u16
            } else {
                return None;
            }
        }
        _ => return None,
    })
}

// ─── KeymapConfig ────────────────────────────────────────────────────────────

/// MS-IME 準拠のデフォルトキーバインド
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeymapConfig {
    #[serde(default)]
    pub preset: Option<KeymapPreset>,
    #[serde(default = "default_inherit_preset")]
    pub inherit_preset: bool,
    #[serde(default)]
    pub bindings: Vec<KeyBinding>,
}

fn default_inherit_preset() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeymapPreset {
    MsImeUs,
    MsImeJis,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyBinding {
    pub key: String,
    pub action: KeyAction,
}

impl Default for KeymapConfig {
    fn default() -> Self {
        Self {
            preset: Some(KeymapPreset::MsImeJis),
            inherit_preset: true,
            bindings: Vec::new(),
        }
    }
}

fn bind(key: &str, action: KeyAction) -> KeyBinding {
    KeyBinding {
        key: key.to_string(),
        action,
    }
}

// ─── Keymap ──────────────────────────────────────────────────────────────────

pub struct Keymap {
    table: HashMap<KeySpec, KeyAction>,
}

impl Keymap {
    /// keymap.toml を必ず読み込む（Activate 用）。失敗時は既定 keymap。
    /// 読み込み後の mtime を `reload_if_changed` の基準として記録する。
    pub fn load() -> Self {
        let km = match load_from_file() {
            Ok(km) => {
                tracing::info!("keymap loaded");
                km
            }
            Err(e) => {
                tracing::warn!("keymap: load failed, using default ({e})");
                Self::default()
            }
        };
        if let Ok(mut r) = KEYMAP_RELOADER.lock() {
            r.mark_loaded();
        }
        km
    }

    /// 入力モード切替時用: keymap.toml が前回から変わっていた場合だけ読み直す。
    ///
    /// 変化が無ければファイル I/O は `metadata` 1 回だけで `None` を返す。
    /// 更新・作成なら新しい keymap、削除なら既定 keymap、parse 失敗なら `None`
    /// （呼び出し側は直前の keymap を維持する）。
    pub fn reload_if_changed() -> Option<Self> {
        let mut r = match KEYMAP_RELOADER.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        r.reload_if_changed()
    }

    fn build(cfg: KeymapConfig) -> Self {
        let mut table = HashMap::new();
        for b in &cfg.bindings {
            if let Some(spec) = KeySpec::parse(&b.key) {
                table.insert(spec, b.action.clone());
            } else {
                tracing::warn!("keymap: cannot parse {:?}", b.key);
            }
        }
        Self { table }
    }

    /// ホットパス — HashMap::get のみ
    pub fn resolve(&self, vk: u16, ctrl: bool, shift: bool, alt: bool) -> Option<&KeyAction> {
        self.table.get(&KeySpec {
            vk,
            ctrl,
            shift,
            alt,
        })
    }

    /// VK + 現在の修飾キー状態 → UserAction
    ///
    /// キーマップにあればそのアクション、なければ ToUnicode で文字変換。
    pub fn resolve_action(&self, vk: u16) -> Option<UserAction> {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            GetKeyState, GetKeyboardState, ToUnicode, VK_CONTROL, VK_MENU, VK_SHIFT,
        };
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000 != 0 };
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) as u16 & 0x8000 != 0 };
        let alt = unsafe { GetKeyState(VK_MENU.0 as i32) as u16 & 0x8000 != 0 };
        let space_down = unsafe { GetKeyState(0x20) as u16 & 0x8000 != 0 };
        let (vk, ctrl, shift, alt) = normalize_key_event(vk, ctrl, shift, alt, space_down);

        // ① キーマップ優先
        if let Some(ka) = self.resolve(vk, ctrl, shift, alt) {
            return Some(ka.to_user_action());
        }

        // ①.5 重要キーは設定ファイルが壊れていても確実に動くようにフォールバックを持つ
        // （VK_RETURN は ToUnicode で制御文字になりやすく、Input に変換されないため）。
        // OnTestKeyDown の keymap 取得失敗時と同じ集合を使う（両者の判定を一致させる）。
        if let Some(action) = essential_fallback_action(vk) {
            return Some(action);
        }

        // ② 数字キー（修飾なし）→ 候補選択モード中のみ候補番号選択
        //    選択モード外では ToUnicode に落として通常文字として入力する
        if !ctrl && !alt && super::state::session_is_selecting_fast() {
            let n = match vk {
                0x31..=0x39 => Some(vk - 0x30), // 1–9
                0x61..=0x69 => Some(vk - 0x60), // テンキー 1–9
                _ => None,
            };
            if let Some(n) = n {
                return Some(UserAction::CandidateSelect(n as u8));
            }
        }

        // ② テンキー記号 → ローマ字変換を経由せず直接入力（InputRaw）
        // ToUnicode を通すと JIS かなルールで ・ ー 。等に変換されてしまうため先に処理する
        // 実測 (/*-+. の順に入力): 0x6f=/ 0x6a=* 0x6d=- 0x6b=+ 0x6e=.
        if !ctrl && !alt {
            let ch = match vk {
                0x6F => Some('/'), // テンキー /
                0x6A => Some('*'), // テンキー *
                0x6D => Some('-'), // テンキー -
                0x6B => Some('+'), // テンキー +
                0x6E => Some('.'), // テンキー .
                _ => None,
            };
            if let Some(ch) = ch {
                return Some(UserAction::InputRaw(ch));
            }
        }

        // ③ ToUnicode で文字変換（ローマ字入力）
        let key_state = {
            let mut state = [0u8; 256];
            unsafe { GetKeyboardState(&mut state).ok()? };
            state
        };
        let mut buf = [0u16; 2];
        let n = unsafe { ToUnicode(vk as u32, 0, Some(&key_state), &mut buf, 0) };
        if n > 0 {
            let c = buf[0];
            if c >= 0x20 && !(0x7F..=0x9F).contains(&c) {
                if let Some(ch) = char::from_u32(c as u32) {
                    // Shift+アルファベット → 全角大文字（Ａ–Ｚ）でプリエディットに追加
                    // F9/F10 サイクルが効くよう Input で送り、factory 側で専用メソッドを呼ぶ
                    if shift && !ctrl && !alt && ch.is_ascii_uppercase() {
                        return Some(UserAction::Input(ch));
                    }
                    // 全ての印字可能文字を Input として push_char に委ねる。
                    return Some(UserAction::Input(ch));
                }
            }
        }

        // ④ その他
        match vk {
            0x09 => Some(UserAction::Tab),
            _ => None,
        }
    }
}

impl Default for Keymap {
    fn default() -> Self {
        let preset = match super::config::keyboard_layout() {
            super::config::KeyboardLayout::Us => KeymapPreset::MsImeUs,
            super::config::KeyboardLayout::Jis | super::config::KeyboardLayout::Custom => {
                KeymapPreset::MsImeJis
            }
        };
        Self::build(resolve_keymap_config(KeymapConfig {
            preset: Some(preset),
            inherit_preset: true,
            bindings: Vec::new(),
        }))
    }
}

// ─── 設定ファイル ─────────────────────────────────────────────────────────────

pub fn keymap_save_default() -> Result<()> {
    let path = config_path()?;
    if !path.exists() {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let header = concat!(
            "# rakukan キーバインド設定\n",
            "# IME をオフ→オンにすると反映されます\n",
            "#\n",
            "# action の種類:\n",
            "#   [プリエディット]\n",
            "#     convert           -- 変換開始 (Space, 変換キー)\n",
            "#     commit_raw        -- ひらがなのまま確定 (Enter)\n",
            "#     backspace         -- 1文字削除\n",
            "#     cancel            -- 変換取り消し / プリエディット破棄 (Escape)\n",
            "#     cancel_all        -- プリエディット全破棄 (Ctrl+Backspace)\n",
            "#     hiragana          -- ひらがな変換 (F6)\n",
            "#     katakana          -- カタカナ変換 (F7)\n",
            "#     half_katakana     -- 半角カタカナ変換 (F8)\n",
            "#     full_latin        -- 全角英数変換 (F9)\n",
            "#     half_latin        -- 半角英数変換 (F10)\n",
            "#     cycle_kana        -- ひらがな→カタカナ→半角カタカナ 循環 (無変換)\n",
            "#     full_width_space  -- 全角スペース入力 (Shift+Space)\n",
            "#   [候補ウィンドウ]\n",
            "#     candidate_next      -- 次の候補 (↓)\n",
            "#     candidate_prev      -- 前の候補 (↑)\n",
            "#     candidate_page_down -- 次ページ (Tab, PageDown)\n",
            "#     candidate_page_up   -- 前ページ (Shift+Tab, PageUp)\n",
            "#   [IME オン/オフ]\n",
            "#     ime_toggle        -- オン↔オフ切り替え (全角/半角)\n",
            "#     ime_off           -- IME をオフ (英数パススルー)\n",
            "#     ime_on            -- IME をオン (ひらがなモードへ)\n",
            "#   [入力モード切り替え]\n",
            "#     mode_hiragana     -- ひらがなモードへ\n",
            "#     mode_katakana     -- カタカナモードへ (全角)\n",
            "#     mode_alphanumeric -- 英数モードへ\n",
            "#\n",
            "# キー名:\n",
            "#   通常キー : Enter, Space, Escape, Backspace, Tab, Delete\n",
            "#   矢印キー : Left, Up, Right, Down\n",
            "#   ファンクション: F1 - F12\n",
            "#   ページ   : PageUp, PageDown, Home, End\n",
            "#   日本語キー (日本語キーボードのみ):\n",
            "#     Zenkaku      -- 全角/半角\n",
            "#     Henkan       -- 変換\n",
            "#     Muhenkan     -- 無変換\n",
            "#     Hiragana_key -- ひらがな\n",
            "#     Katakana     -- カタカナ\n",
            "#     Eisuu        -- 英数\n",
            "#     Caps         -- Caps Lock\n",
            "#   修飾キー : Ctrl+, Shift+, Alt+（組み合わせ可）\n",
            "#   例: \"Ctrl+Space\", \"Shift+Tab\", \"Alt+Caps\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Space\"\n",
            "action = \"convert\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Enter\"\n",
            "action = \"commit_raw\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Henkan\"\n",
            "action = \"convert\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Escape\"\n",
            "action = \"cancel\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Ctrl+Backspace\"\n",
            "action = \"cancel_all\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Backspace\"\n",
            "action = \"backspace\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"F6\"\n",
            "action = \"hiragana\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"F7\"\n",
            "action = \"katakana\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"F8\"\n",
            "action = \"half_katakana\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"F9\"\n",
            "action = \"full_latin\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"F10\"\n",
            "action = \"half_latin\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Muhenkan\"\n",
            "action = \"cycle_kana\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Shift+Space\"\n",
            "action = \"full_width_space\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Tab\"\n",
            "action = \"candidate_page_down\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Down\"\n",
            "action = \"candidate_next\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Shift+Tab\"\n",
            "action = \"candidate_page_up\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Up\"\n",
            "action = \"candidate_prev\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"PageDown\"\n",
            "action = \"candidate_page_down\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"PageUp\"\n",
            "action = \"candidate_page_up\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Zenkaku\"\n",
            "action = \"ime_toggle\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Ctrl+Space\"\n",
            "action = \"ime_toggle\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Hiragana_key\"\n",
            "action = \"mode_hiragana\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Ctrl+Caps\"\n",
            "action = \"mode_hiragana\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Katakana\"\n",
            "action = \"mode_katakana\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Alt+Caps\"\n",
            "action = \"mode_katakana\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Eisuu\"\n",
            "action = \"mode_alphanumeric\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Left\"\n",
            "action = \"cursor_left\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Right\"\n",
            "action = \"cursor_right\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"Home\"\n",
            "action = \"cursor_home\"\n",
            "\n",
            "[[bindings]]\n",
            "key    = \"End\"\n",
            "action = \"cursor_end\"\n",
            "\n",
        );
        std::fs::write(&path, header)?;
        tracing::info!("keymap.toml created: {}", path.display());
    }
    Ok(())
}

fn config_path() -> Result<std::path::PathBuf> {
    let appdata = std::env::var("APPDATA").map_err(|_| anyhow::anyhow!("APPDATA not set"))?;
    Ok(std::path::PathBuf::from(appdata)
        .join("rakukan")
        .join("keymap.toml"))
}

fn load_from_file() -> Result<Keymap> {
    load_keymap_from_path(&config_path()?)
}

fn load_keymap_from_path(path: &Path) -> Result<Keymap> {
    let text = std::fs::read_to_string(path)?;
    let cfg: KeymapConfig = toml::from_str(&text)?;
    Ok(Keymap::build(resolve_keymap_config(cfg)))
}

// ─── 変更検出（mtime gate）────────────────────────────────────────────────────

/// keymap.toml の変更種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeymapChange {
    Unchanged,
    Created,
    Updated,
    Deleted,
}

/// 前回記録した mtime と現在の mtime から変更種別を判定する（純粋関数）。
/// `None` はファイルが存在しない（または mtime を取れない）ことを表す。
pub fn classify_change(prev: Option<SystemTime>, now: Option<SystemTime>) -> KeymapChange {
    match (prev, now) {
        (a, b) if a == b => KeymapChange::Unchanged,
        (None, Some(_)) => KeymapChange::Created,
        (Some(_), None) => KeymapChange::Deleted,
        (Some(_), Some(_)) => KeymapChange::Updated,
        (None, None) => KeymapChange::Unchanged,
    }
}

fn file_modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// keymap.toml の path と最終更新時刻を保持し、変わったときだけ読み直す。
///
/// config 側の `ConfigManager::reload_if_changed` と同じ方針だが、config と keymap の
/// 失敗を互いに巻き込まないよう独立して持つ。
#[derive(Debug)]
pub struct KeymapReloader {
    path: Option<PathBuf>,
    last_modified: Option<SystemTime>,
}

impl KeymapReloader {
    /// 現在の mtime を基準として開始する。`path` が `None` なら何もしない reloader になる。
    pub fn new(path: Option<PathBuf>) -> Self {
        let last_modified = path.as_deref().and_then(file_modified);
        Self {
            path,
            last_modified,
        }
    }

    /// 読み込み直後に呼び、現在の mtime を基準にする。
    pub fn mark_loaded(&mut self) {
        self.last_modified = self.path.as_deref().and_then(file_modified);
    }

    pub fn reload_if_changed(&mut self) -> Option<Keymap> {
        self.reload_if_changed_with(load_keymap_from_path)
    }

    /// 変更があれば `load(path)` で読み直す。テストで実ファイルの parse を差し替えられるよう
    /// ロード処理は注入する。
    ///
    /// - `Unchanged`: `None`（ファイル I/O は `metadata` のみ）
    /// - `Created` / `Updated`: 成功なら `Some(新 keymap)`。parse 失敗なら WARN を出して
    ///   `None`（直前の keymap を維持）。どちらも mtime は更新し、壊れたファイルを
    ///   切替ごとに parse し直さない
    /// - `Deleted`: `Some(既定 keymap)`
    pub fn reload_if_changed_with(
        &mut self,
        load: impl FnOnce(&Path) -> Result<Keymap>,
    ) -> Option<Keymap> {
        let path = self.path.as_deref()?;
        let now = file_modified(path);
        let change = classify_change(self.last_modified, now);
        if change == KeymapChange::Unchanged {
            return None;
        }
        self.last_modified = now;
        match change {
            KeymapChange::Deleted => {
                tracing::info!("keymap.toml deleted; using default keymap");
                Some(Keymap::default())
            }
            KeymapChange::Created | KeymapChange::Updated => match load(path) {
                Ok(km) => {
                    tracing::info!("keymap reloaded ({change:?}): {}", path.display());
                    Some(km)
                }
                Err(e) => {
                    tracing::warn!(
                        "keymap.toml changed but failed to load; keeping previous keymap: {e}"
                    );
                    None
                }
            },
            KeymapChange::Unchanged => None,
        }
    }
}

static KEYMAP_RELOADER: LazyLock<Mutex<KeymapReloader>> =
    LazyLock::new(|| Mutex::new(KeymapReloader::new(config_path().ok())));

fn resolve_keymap_config(mut cfg: KeymapConfig) -> KeymapConfig {
    let layout_preset = match super::config::keyboard_layout() {
        super::config::KeyboardLayout::Us => KeymapPreset::MsImeUs,
        super::config::KeyboardLayout::Jis | super::config::KeyboardLayout::Custom => {
            KeymapPreset::MsImeJis
        }
    };
    let preset = cfg.preset.unwrap_or(layout_preset);
    if !cfg.inherit_preset || matches!(preset, KeymapPreset::Custom) {
        return cfg;
    }

    let mut bindings = preset_bindings(preset);
    bindings.extend(cfg.bindings);
    cfg.bindings = bindings;
    cfg
}

fn preset_bindings(preset: KeymapPreset) -> Vec<KeyBinding> {
    match preset {
        KeymapPreset::MsImeUs => vec![
            bind("Ctrl+Space", KeyAction::ImeToggle),
            bind("Ctrl+J", KeyAction::ModeHiragana),
            bind("Ctrl+K", KeyAction::ModeKatakana),
            bind("Ctrl+L", KeyAction::ModeAlphanumeric),
            bind("Space", KeyAction::Convert),
            bind("Enter", KeyAction::CommitRaw),
            bind("Escape", KeyAction::Cancel),
            bind("Ctrl+Backspace", KeyAction::CancelAll),
            bind("Backspace", KeyAction::Backspace),
            bind("F6", KeyAction::Hiragana),
            bind("F7", KeyAction::Katakana),
            bind("F8", KeyAction::HalfKatakana),
            bind("F9", KeyAction::FullLatin),
            bind("F10", KeyAction::HalfLatin),
            bind("Shift+Space", KeyAction::FullWidthSpace),
            bind("Down", KeyAction::CandidateNext),
            bind("Up", KeyAction::CandidatePrev),
            bind("Tab", KeyAction::CandidatePageDown),
            bind("Shift+Tab", KeyAction::CandidatePageUp),
            bind("PageDown", KeyAction::CandidatePageDown),
            bind("PageUp", KeyAction::CandidatePageUp),
            bind("Left", KeyAction::CursorLeft),
            bind("Right", KeyAction::CursorRight),
            bind("Home", KeyAction::CursorHome),
            bind("End", KeyAction::CursorEnd),
            bind("Shift+Left", KeyAction::SegmentShrink),
            bind("Shift+Right", KeyAction::SegmentExtend),
        ],
        KeymapPreset::MsImeJis => vec![
            bind("Space", KeyAction::Convert),
            bind("Enter", KeyAction::CommitRaw),
            bind("Henkan", KeyAction::Convert),
            bind("Escape", KeyAction::Cancel),
            bind("Ctrl+Backspace", KeyAction::CancelAll),
            bind("Backspace", KeyAction::Backspace),
            bind("F6", KeyAction::Hiragana),
            bind("F7", KeyAction::Katakana),
            bind("F8", KeyAction::HalfKatakana),
            bind("F9", KeyAction::FullLatin),
            bind("F10", KeyAction::HalfLatin),
            bind("Muhenkan", KeyAction::CycleKana),
            bind("Shift+Space", KeyAction::FullWidthSpace),
            bind("Down", KeyAction::CandidateNext),
            bind("Up", KeyAction::CandidatePrev),
            bind("Tab", KeyAction::CandidatePageDown),
            bind("Shift+Tab", KeyAction::CandidatePageUp),
            bind("PageDown", KeyAction::CandidatePageDown),
            bind("PageUp", KeyAction::CandidatePageUp),
            bind("Zenkaku", KeyAction::ImeToggle),
            bind("Ctrl+Space", KeyAction::ImeToggle),
            bind("Hiragana_key", KeyAction::ModeHiragana),
            bind("Ctrl+Caps", KeyAction::ModeHiragana),
            bind("Katakana", KeyAction::ModeKatakana),
            bind("Alt+Caps", KeyAction::ModeKatakana),
            bind("Eisuu", KeyAction::ModeAlphanumeric),
            bind("Left", KeyAction::CursorLeft),
            bind("Right", KeyAction::CursorRight),
            bind("Home", KeyAction::CursorHome),
            bind("End", KeyAction::CursorEnd),
            bind("Shift+Left", KeyAction::SegmentShrink),
            bind("Shift+Right", KeyAction::SegmentExtend),
        ],
        KeymapPreset::Custom => Vec::new(),
    }
}

/// キーイベントの VK / 修飾キーを keymap 照合前に正規化する（純粋関数）。
///
/// `OnTestKeyDown` / `OnKeyDown`（`factory.rs`）と `resolve_action` の両方がこれを通るので、
/// VK の読み替えはここに集約する。
///
/// - Ctrl+Alt+Right（Space 押下中）→ Ctrl+Space: Windows の一部環境で Ctrl+Space が
///   Ctrl+Alt+Right として通知されることがあるため、IME トグルの別名として吸収する。
/// - `VK_DBE_SBCSCHAR`（0xF3）/ `VK_DBE_DBCSCHAR`（0xF4）→ `VK_KANJI`（0x19）:
///   JIS 配列の半角/全角キーは環境によって 0x19 ではなく 0xF3 / 0xF4（現在の IME 状態で
///   どちらかになる）を送る。keymap は `Zenkaku = 0x19` で持っているので 0x19 に寄せる。
///   修飾キーはそのまま渡し、Shift や Ctrl 併用時の扱いは keymap 側の照合に委ねる。
pub(crate) fn normalize_key_event(
    vk: u16,
    ctrl: bool,
    shift: bool,
    alt: bool,
    space_down: bool,
) -> (u16, bool, bool, bool) {
    if vk == 0x27 && ctrl && alt && !shift && space_down {
        return (0x20, true, false, false);
    }
    if vk == 0xF3 || vk == 0xF4 {
        return (0x19, ctrl, shift, alt);
    }
    (vk, ctrl, shift, alt)
}

/// keymap で解決できなかった場合でも必ず動かす重要キー。
///
/// `resolve_action` の ①.5（keymap に binding が無い / 設定ファイルが壊れている）と、
/// `OnTestKeyDown` の keymap 取得失敗時（RefCell 競合）の両方で同じ集合を使う。
/// 半角/全角（`VK_KANJI`）が入っているのは、keymap が壊れていても IME の ON/OFF だけは
/// できるようにするため。
pub(crate) fn essential_fallback_action(vk: u16) -> Option<UserAction> {
    match vk {
        0x0D => Some(UserAction::CommitRaw), // VK_RETURN
        0x20 => Some(UserAction::Convert),   // VK_SPACE
        0x08 => Some(UserAction::Backspace), // VK_BACK
        0x1B => Some(UserAction::Cancel),    // VK_ESCAPE
        0x1A => Some(UserAction::ImeOff),    // VK_IME_OFF
        0x16 => Some(UserAction::ImeOn),     // VK_IME_ON
        0x19 => Some(UserAction::ImeToggle), // VK_KANJI（半角/全角）
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_binding_overrides_preset_binding() {
        let cfg = resolve_keymap_config(KeymapConfig {
            preset: Some(KeymapPreset::MsImeJis),
            inherit_preset: true,
            bindings: vec![KeyBinding {
                key: "Ctrl+Space".to_string(),
                action: KeyAction::ModeAlphanumeric,
            }],
        });
        let keymap = Keymap::build(cfg);
        let action = keymap.resolve(0x20, true, false, false);
        assert_eq!(action, Some(&KeyAction::ModeAlphanumeric));
    }

    #[test]
    fn custom_preset_disables_inherited_defaults() {
        let cfg = resolve_keymap_config(KeymapConfig {
            preset: Some(KeymapPreset::Custom),
            inherit_preset: false,
            bindings: vec![KeyBinding {
                key: "F6".to_string(),
                action: KeyAction::Hiragana,
            }],
        });
        let keymap = Keymap::build(cfg);
        assert_eq!(
            keymap.resolve(0x75, false, false, false),
            Some(&KeyAction::Hiragana)
        );
        assert_eq!(keymap.resolve(0x20, false, false, false), None);
    }

    #[test]
    fn normalize_jis_zenkaku_hankaku_to_vk_kanji() {
        // JIS 配列の半角/全角キー: VK_DBE_SBCSCHAR / VK_DBE_DBCSCHAR → VK_KANJI
        assert_eq!(
            normalize_key_event(0xF3, false, false, false, false),
            (0x19, false, false, false)
        );
        assert_eq!(
            normalize_key_event(0xF4, false, false, false, false),
            (0x19, false, false, false)
        );
        // VK_KANJI はそのまま
        assert_eq!(
            normalize_key_event(0x19, false, false, false, false),
            (0x19, false, false, false)
        );
        // 修飾キーは変更せず keymap 側に委ねる
        assert_eq!(
            normalize_key_event(0xF3, true, true, false, false),
            (0x19, true, true, false)
        );
    }

    #[test]
    fn normalize_leaves_unrelated_vks_unchanged() {
        for vk in [0x0D_u16, 0x20, 0x41, 0x5A, 0x70, 0xF0, 0xF1, 0xF2, 0xF5] {
            assert_eq!(
                normalize_key_event(vk, false, false, false, false),
                (vk, false, false, false),
                "vk={vk:#04x}"
            );
        }
    }

    #[test]
    fn default_preset_resolves_normalized_zenkaku_to_ime_toggle() {
        let cfg = resolve_keymap_config(KeymapConfig {
            preset: Some(KeymapPreset::MsImeJis),
            inherit_preset: true,
            bindings: vec![],
        });
        let keymap = Keymap::build(cfg);
        for raw in [0xF3_u16, 0xF4, 0x19] {
            let (vk, ctrl, shift, alt) = normalize_key_event(raw, false, false, false, false);
            assert_eq!(
                keymap.resolve(vk, ctrl, shift, alt),
                Some(&KeyAction::ImeToggle),
                "raw={raw:#04x}"
            );
        }
    }

    #[test]
    fn essential_fallback_covers_ime_toggle_keys() {
        assert_eq!(essential_fallback_action(0x19), Some(UserAction::ImeToggle));
        assert_eq!(essential_fallback_action(0x1A), Some(UserAction::ImeOff));
        assert_eq!(essential_fallback_action(0x16), Some(UserAction::ImeOn));
        assert_eq!(essential_fallback_action(0x0D), Some(UserAction::CommitRaw));
        assert_eq!(essential_fallback_action(0x41), None);
        assert_eq!(
            essential_fallback_action(0xF3),
            None,
            "正規化前の VK は対象外"
        );
    }

    #[test]
    fn default_presets_bind_home_and_end_to_cursor_jump() {
        for preset in [KeymapPreset::MsImeJis, KeymapPreset::MsImeUs] {
            let cfg = resolve_keymap_config(KeymapConfig {
                preset: Some(preset),
                inherit_preset: true,
                bindings: vec![],
            });
            let keymap = Keymap::build(cfg);
            assert_eq!(
                keymap.resolve(0x24, false, false, false),
                Some(&KeyAction::CursorHome),
                "{preset:?} Home"
            );
            assert_eq!(
                keymap.resolve(0x23, false, false, false),
                Some(&KeyAction::CursorEnd),
                "{preset:?} End"
            );
        }
    }

    #[test]
    fn cursor_home_end_parse_from_keymap_toml() {
        let cfg: KeymapConfig = toml::from_str(
            "preset = \"custom\"\ninherit_preset = false\n[[bindings]]\nkey = \"Home\"\naction = \"cursor_home\"\n[[bindings]]\nkey = \"End\"\naction = \"cursor_end\"\n",
        )
        .unwrap();
        let keymap = Keymap::build(resolve_keymap_config(cfg));
        assert_eq!(
            keymap.resolve(0x24, false, false, false),
            Some(&KeyAction::CursorHome)
        );
        assert_eq!(
            keymap.resolve(0x23, false, false, false),
            Some(&KeyAction::CursorEnd)
        );
        assert_eq!(
            KeyAction::CursorHome.to_user_action(),
            UserAction::CursorHome
        );
        assert_eq!(KeyAction::CursorEnd.to_user_action(), UserAction::CursorEnd);
    }

    #[test]
    fn classify_change_covers_all_transitions() {
        let t1 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        let t2 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2);
        assert_eq!(classify_change(None, None), KeymapChange::Unchanged);
        assert_eq!(classify_change(Some(t1), Some(t1)), KeymapChange::Unchanged);
        assert_eq!(classify_change(None, Some(t1)), KeymapChange::Created);
        assert_eq!(classify_change(Some(t1), None), KeymapChange::Deleted);
        assert_eq!(classify_change(Some(t1), Some(t2)), KeymapChange::Updated);
    }

    /// テスト用の一時 keymap.toml。Drop で削除する。
    struct TempKeymap(PathBuf);

    impl TempKeymap {
        fn create(name: &str, body: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "rakukan-keymap-test-{}-{name}.toml",
                std::process::id()
            ));
            std::fs::write(&path, body).unwrap();
            Self(path)
        }

        /// 内容を書き換え、mtime を確実に前へ進める（同一秒内の書き込みでも検出できるように）。
        fn rewrite(&self, body: &str) {
            std::fs::write(&self.0, body).unwrap();
            let f = std::fs::File::options().write(true).open(&self.0).unwrap();
            let bumped = std::fs::metadata(&self.0).unwrap().modified().unwrap()
                + std::time::Duration::from_secs(5);
            f.set_modified(bumped).unwrap();
        }
    }

    impl Drop for TempKeymap {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    const F6_HIRAGANA: &str = "preset = \"custom\"\ninherit_preset = false\n[[bindings]]\nkey = \"F6\"\naction = \"hiragana\"\n";
    const F6_KATAKANA: &str = "preset = \"custom\"\ninherit_preset = false\n[[bindings]]\nkey = \"F6\"\naction = \"katakana\"\n";

    #[test]
    fn reloader_does_not_reload_when_mtime_is_unchanged() {
        let tmp = TempKeymap::create("unchanged", F6_HIRAGANA);
        let mut r = KeymapReloader::new(Some(tmp.0.clone()));
        let mut loads = 0;
        let got = r.reload_if_changed_with(|p| {
            loads += 1;
            load_keymap_from_path(p)
        });
        assert!(got.is_none());
        assert_eq!(loads, 0, "mtime 不変なら parse しない");
    }

    #[test]
    fn reloader_reloads_updated_file_with_new_bindings() {
        let tmp = TempKeymap::create("updated", F6_HIRAGANA);
        let mut r = KeymapReloader::new(Some(tmp.0.clone()));
        tmp.rewrite(F6_KATAKANA);
        let km = r
            .reload_if_changed_with(load_keymap_from_path)
            .expect("更新を検出する");
        assert_eq!(
            km.resolve(0x75, false, false, false),
            Some(&KeyAction::Katakana)
        );
        // 2 回目は変化なし
        assert!(r.reload_if_changed_with(load_keymap_from_path).is_none());
    }

    #[test]
    fn reloader_keeps_previous_keymap_when_parse_fails_and_does_not_retry() {
        let tmp = TempKeymap::create("broken", F6_HIRAGANA);
        let mut r = KeymapReloader::new(Some(tmp.0.clone()));
        tmp.rewrite("this is not toml = = =");
        let mut loads = 0;
        let got = r.reload_if_changed_with(|p| {
            loads += 1;
            load_keymap_from_path(p)
        });
        assert!(got.is_none(), "parse 失敗は None（直前の keymap を維持）");
        assert_eq!(loads, 1);
        // 壊れたままの同じファイルを次の切替で parse し直さない
        let got2 = r.reload_if_changed_with(|p| {
            loads += 1;
            load_keymap_from_path(p)
        });
        assert!(got2.is_none());
        assert_eq!(loads, 1);
    }

    #[test]
    fn reloader_falls_back_to_default_when_file_is_deleted() {
        let tmp = TempKeymap::create("deleted", F6_HIRAGANA);
        let mut r = KeymapReloader::new(Some(tmp.0.clone()));
        std::fs::remove_file(&tmp.0).unwrap();
        let km = r
            .reload_if_changed_with(|_| panic!("削除時は load を呼ばない"))
            .expect("削除は既定 keymap を返す");
        // 既定プリセットは Space=Convert を持つ
        assert_eq!(
            km.resolve(0x20, false, false, false),
            Some(&KeyAction::Convert)
        );
    }

    #[test]
    fn reloader_without_path_never_reloads() {
        let mut r = KeymapReloader::new(None);
        assert!(r.reload_if_changed_with(|_| panic!("呼ばれない")).is_none());
    }

    #[test]
    fn normalize_ctrl_alt_right_aliases_ctrl_space() {
        assert_eq!(
            normalize_key_event(0x27, true, false, true, true),
            (0x20, true, false, false)
        );
        assert_eq!(
            normalize_key_event(0x27, true, false, true, false),
            (0x27, true, false, true)
        );
    }
}
