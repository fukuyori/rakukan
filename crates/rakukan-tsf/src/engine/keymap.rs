//! キーバインド設定（MS-IME 準拠デフォルト）
//!
//! 設定ファイル: `%APPDATA%\rakukan\keymap.toml`
//! リロードタイミング: IME オフ → オン（Activate）のみ。

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::user_action::UserAction;

// ─── KeyAction ───────────────────────────────────────────────────────────────

/// 設定ファイルに書けるアクション名
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    Convert,           // Space, 変換キー
    CommitRaw,         // Enter（ひらがなのまま確定）
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
    ImeOff,            // 英数キー（IME オン中）
    ImeOn,             // 英数キー以外（IME オフ中）
    ImeToggle,         // 全角/半角, Ctrl+Space
    // 入力モード切り替え（IME オン中）
    ModeHiragana,      // ひらがなキー, Ctrl+Caps
    ModeKatakana,      // カタカナキー, Alt+Caps
    ModeAlphanumeric,  // 英数キー
    CursorLeft,
    CursorRight,
}

impl KeyAction {
    pub fn to_user_action(&self) -> UserAction {
        match self {
            Self::Convert          => UserAction::Convert,
            Self::CommitRaw        => UserAction::CommitRaw,
            Self::Backspace        => UserAction::Backspace,
            Self::CancelAll        => UserAction::CancelAll,
            Self::Cancel           => UserAction::Cancel,
            Self::Hiragana         => UserAction::Hiragana,
            Self::Katakana         => UserAction::Katakana,
            Self::HalfKatakana     => UserAction::HalfKatakana,
            Self::FullLatin        => UserAction::FullLatin,
            Self::HalfLatin        => UserAction::HalfLatin,
            Self::CycleKana        => UserAction::CycleKana,
            Self::FullWidthSpace   => UserAction::FullWidthSpace,
            Self::CandidateNext    => UserAction::CandidateNext,
            Self::CandidatePrev    => UserAction::CandidatePrev,
            Self::CandidatePageDown => UserAction::CandidatePageDown,
            Self::CandidatePageUp  => UserAction::CandidatePageUp,
            Self::CandidateN(n)    => UserAction::CandidateSelect(*n),
            Self::ImeOff          => UserAction::ImeOff,
            Self::ImeOn            => UserAction::ImeOn,
            Self::ImeToggle        => UserAction::ImeToggle,
            Self::ModeHiragana     => UserAction::ModeHiragana,
            Self::ModeKatakana     => UserAction::ModeKatakana,
            Self::ModeAlphanumeric => UserAction::ModeAlphanumeric,
            Self::CursorLeft       => UserAction::CursorLeft,
            Self::CursorRight      => UserAction::CursorRight,
        }
    }
}

// ─── KeySpec ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeySpec {
    pub vk:    u16,
    pub ctrl:  bool,
    pub shift: bool,
    pub alt:   bool,
}

impl KeySpec {
    pub fn parse(s: &str) -> Option<Self> {
        let mut ctrl  = false;
        let mut shift = false;
        let mut alt   = false;
        let mut vk: Option<u16> = None;
        for part in s.split('+') {
            match part.trim().to_lowercase().as_str() {
                "ctrl" | "control" => ctrl  = true,
                "shift"            => shift = true,
                "alt"              => alt   = true,
                name => vk = Some(name_to_vk(name)?),
            }
        }
        Some(Self { vk: vk?, ctrl, shift, alt })
    }
}

fn name_to_vk(name: &str) -> Option<u16> {
    Some(match name {
        "backspace" | "bs"    => 0x08,
        "tab"                 => 0x09,
        "enter" | "return"    => 0x0D,
        "escape" | "esc"      => 0x1B,
        "space"               => 0x20,
        "backquote" | "grave" => 0xC0,
        "semicolon"           => 0xBA,
        "equal"               => 0xBB,
        "comma"               => 0xBC,
        "minus"               => 0xBD,
        "period"              => 0xBE,
        "slash"               => 0xBF,
        "leftbracket"         => 0xDB,
        "backslash"           => 0xDC,
        "rightbracket"        => 0xDD,
        "quote"               => 0xDE,
        "pageup"   | "pgup"   => 0x21,
        "pagedown" | "pgdn"   => 0x22,
        "end"                 => 0x23,
        "home"                => 0x24,
        "left"                => 0x25,
        "up"                  => 0x26,
        "right"               => 0x27,
        "down"                => 0x28,
        "delete" | "del"      => 0x2E,
        "f1"  => 0x70, "f2"  => 0x71, "f3"  => 0x72,
        "f4"  => 0x73, "f5"  => 0x74, "f6"  => 0x75,
        "f7"  => 0x76, "f8"  => 0x77, "f9"  => 0x78,
        "f10" => 0x79, "f11" => 0x7A, "f12" => 0x7B,
        "zenkaku" | "hankaku" | "kanji" => 0xF3,
        "henkan"              => 0x1C,
        "muhenkan"            => 0x1D,
        "eisuu" | "alphanumeric" => 0xF0,  // 英数キー
        "katakana"               => 0xF1,  // カタカナキー
        "hiragana_key"           => 0xF2,  // ひらがなキー
        "caps"                   => 0x14,  // Caps Lock
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

fn default_inherit_preset() -> bool { true }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeymapPreset {
    MsImeUs,
    MsImeJis,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyBinding {
    pub key:    String,
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
    KeyBinding { key: key.to_string(), action }
}

// ─── Keymap ──────────────────────────────────────────────────────────────────

pub struct Keymap {
    table: HashMap<KeySpec, KeyAction>,
}

impl Keymap {
    pub fn load() -> Self {
        match load_from_file() {
            Ok(km) => { tracing::info!("keymap loaded"); km }
            Err(e) => { tracing::info!("keymap default ({e})"); Self::default() }
        }
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
        self.table.get(&KeySpec { vk, ctrl, shift, alt })
    }

    /// VK + 現在の修飾キー状態 → UserAction
    ///
    /// キーマップにあればそのアクション、なければ ToUnicode で文字変換。
    pub fn resolve_action(&self, vk: u16) -> Option<UserAction> {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            GetKeyboardState, GetKeyState, ToUnicode, VK_CONTROL, VK_MENU, VK_SHIFT,
        };
        let ctrl  = unsafe { GetKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000 != 0 };
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32)   as u16 & 0x8000 != 0 };
        let alt   = unsafe { GetKeyState(VK_MENU.0 as i32)    as u16 & 0x8000 != 0 };

        // ① キーマップ優先
        if let Some(ka) = self.resolve(vk, ctrl, shift, alt) {
            return Some(ka.to_user_action());
        }

        // ①.5 重要キーは設定ファイルが壊れていても確実に動くようにフォールバックを持つ
        // （VK_RETURN は ToUnicode で制御文字になりやすく、Input に変換されないため）
        match vk {
            0x0D => return Some(UserAction::CommitRaw), // VK_RETURN
            0x20 => return Some(UserAction::Convert),   // VK_SPACE
            0x08 => return Some(UserAction::Backspace), // VK_BACK
            0x1B => return Some(UserAction::Cancel),    // VK_ESCAPE
            _ => {}
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
                    // Shift + かなモード → 全角英数字に変換
                    if shift && (0x21u16..=0x7Eu16).contains(&c) {
                        use super::input_mode::InputMode;
                        let mode = super::state::input_mode_get_atomic();
                        if matches!(mode, InputMode::Hiragana | InputMode::Katakana) {
                            let fw = char::from_u32(c as u32 - 0x21 + 0xFF01).unwrap_or(ch);
                            return Some(UserAction::Input(fw));
                        }
                    }
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
            super::config::KeyboardLayout::Jis | super::config::KeyboardLayout::Custom => KeymapPreset::MsImeJis,
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
        if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
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
        );
        std::fs::write(&path, header)?;
        tracing::info!("keymap.toml created: {}", path.display());
    }
    Ok(())
}

fn config_path() -> Result<std::path::PathBuf> {
    let appdata = std::env::var("APPDATA")
        .map_err(|_| anyhow::anyhow!("APPDATA not set"))?;
    Ok(std::path::PathBuf::from(appdata).join("rakukan").join("keymap.toml"))
}

fn load_from_file() -> Result<Keymap> {
    let text = std::fs::read_to_string(config_path()?)?;
    let cfg: KeymapConfig = toml::from_str(&text)?;
    Ok(Keymap::build(resolve_keymap_config(cfg)))
}

fn resolve_keymap_config(mut cfg: KeymapConfig) -> KeymapConfig {
    let layout_preset = match super::config::keyboard_layout() {
        super::config::KeyboardLayout::Us => KeymapPreset::MsImeUs,
        super::config::KeyboardLayout::Jis | super::config::KeyboardLayout::Custom => KeymapPreset::MsImeJis,
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
        ],
        KeymapPreset::Custom => Vec::new(),
    }
}
