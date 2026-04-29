#![windows_subsystem = "windows"]
#![allow(unsafe_op_in_unsafe_fn)]

use anyhow::{Context, Result, anyhow, bail};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};
use toml::{Value, map::Map};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            COLOR_BTNFACE, DEFAULT_GUI_FONT, GetStockObject, GetSysColorBrush, HFONT, SetBkMode,
            TRANSPARENT,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::Controls::{
            ICC_TAB_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, NMHDR, TCIF_TEXT, TCITEMW,
            TCM_ADJUSTRECT, TCM_GETCURSEL, TCM_INSERTITEMW, TCN_SELCHANGE,
        },
        UI::WindowsAndMessaging::{
            BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_GROUPBOX, CB_ADDSTRING,
            CB_GETCURSEL, CB_SETCURSEL, CBS_DROPDOWNLIST, CreateWindowExW, DefWindowProcW,
            DestroyWindow, DispatchMessageW, ES_AUTOHSCROLL, GWLP_USERDATA, GetClientRect,
            GetMessageW, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, HMENU, IDC_ARROW,
            LoadCursorW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MSG, MessageBoxW,
            PostQuitMessage, RegisterClassW, SW_HIDE, SW_SHOW, SendMessageW, SetWindowLongPtrW,
            SetWindowTextW, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE,
            WM_COMMAND, WM_CREATE, WM_CTLCOLORSTATIC, WM_DESTROY, WM_NCCREATE, WM_NCDESTROY,
            WM_NOTIFY, WM_SETFONT, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE,
            WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
        },
    },
    core::{PCWSTR, w},
};

const APP_TITLE: &str = "Rakukan 設定";
const WINDOW_CLASS: &str = "rakukan.settings.window";
const PANEL_CLASS: &str = "rakukan.settings.page";
const WINDOW_WIDTH: i32 = 760;
const WINDOW_HEIGHT: i32 = 860;

const ID_LOG_LEVEL: i32 = 100;
const ID_GPU_BACKEND: i32 = 101;
const ID_N_GPU_LAYERS: i32 = 102;
const ID_MAIN_GPU: i32 = 103;
const ID_MODEL_VARIANT: i32 = 104;
const ID_NUM_CANDIDATES: i32 = 105;
const ID_KEYBOARD_LAYOUT: i32 = 106;
const ID_RELOAD_ON_MODE_SWITCH: i32 = 107;
const ID_DEFAULT_MODE: i32 = 108;
const ID_REMEMBER_LAST_KANA_MODE: i32 = 109;
const ID_DIGIT_WIDTH: i32 = 110;
const ID_LIVE_ENABLED: i32 = 111;
const ID_DEBOUNCE_MS: i32 = 112;
const ID_USE_LLM: i32 = 113;
const ID_PREFER_DICTIONARY_FIRST: i32 = 114;
const ID_BEAM_SIZE: i32 = 115;
const ID_KEYMAP_PRESET: i32 = 116;
const ID_KEYMAP_INHERIT_PRESET: i32 = 117;
const ID_KEYMAP_IME_TOGGLE: i32 = 118;
const ID_KEYMAP_CONVERT: i32 = 119;
const ID_KEYMAP_COMMIT_RAW: i32 = 120;
const ID_KEYMAP_CANCEL: i32 = 121;
const ID_KEYMAP_CANCEL_ALL: i32 = 122;
const ID_KEYMAP_MODE_HIRAGANA: i32 = 123;
const ID_KEYMAP_MODE_KATAKANA: i32 = 124;
const ID_KEYMAP_MODE_ALPHANUMERIC: i32 = 125;
const ID_KEYMAP_CLEAR_IME_TOGGLE: i32 = 126;
const ID_KEYMAP_CLEAR_CONVERT: i32 = 127;
const ID_KEYMAP_CLEAR_COMMIT_RAW: i32 = 128;
const ID_KEYMAP_CLEAR_CANCEL: i32 = 129;
const ID_KEYMAP_CLEAR_CANCEL_ALL: i32 = 130;
const ID_KEYMAP_CLEAR_MODE_HIRAGANA: i32 = 131;
const ID_KEYMAP_CLEAR_MODE_KATAKANA: i32 = 132;
const ID_KEYMAP_CLEAR_MODE_ALPHANUMERIC: i32 = 133;
const ID_OPEN_CONFIG: i32 = 200;
const ID_SAVE: i32 = 201;
const ID_CANCEL: i32 = 202;
const ID_OPEN_KEYMAP: i32 = 203;
const ID_TAB_CONTROL: i32 = 300;

const TAB_COUNT: usize = 5;

const DEFAULT_CONFIG_TEXT: &str = r#"# rakukan 設定ファイル
# 入力モード変更時に再読込されます。

[general]
# ログレベル: error / warn / info / debug / trace
# debug: 開発中の標準。キー入力ごとの状態変化が見える
# info:  通常運用。初期化・確定・モード変更のみ
# trace: 詳細調査時。ループ内・トークン単位まで出力される（低速）
# 環境変数 RAKUKAN_LOG が設定されている場合はそちらが優先される
log_level = "info"

# GPU バックエンド: "auto" / "cuda" / "vulkan" / "cpu"
# "auto"   : インストール済みの DLL から cuda → vulkan → cpu の順で自動選択（デフォルト）
# "cuda"   : NVIDIA GPU (CUDA) ← RTX シリーズ推奨
# "vulkan" : Vulkan 対応 GPU (AMD / Intel / NVIDIA)
# "cpu"    : CPU のみ（GPU なし、VMware 等）
gpu_backend = "auto"

# GPU に載せるレイヤー数
# 0 で CPU のみ、未指定で全レイヤーを GPU にオフロード
# GPU 競合や他アプリの異常終了がある場合は 8 / 16 / 24 など小さめを試す
n_gpu_layers = 16

# 使用する GPU インデックス（複数 GPU 環境で 2 枚目以降を使う場合に変更）
main_gpu = 0

# LLM モデル ID
# jinen-v1-xsmall-q5  : 軽量・推奨（約 30 MB、低スペック PC 向け、デフォルト）
# jinen-v1-small-q5   : 標準（約 84 MB、通常用途）
# jinen-v1-xsmall-f16 : 高精度・大容量（約 138 MB、量子化なし FP16）
# jinen-v1-small-f16  : 高精度・大容量（約 423 MB、量子化なし FP16）
model_variant = "jinen-v1-xsmall-q5"

[keyboard]
layout = "jis"
reload_on_mode_switch = true

[input]
default_mode = "alphanumeric"
remember_last_kana_mode = true
# 数字の入力幅: "halfwidth" = 半角 (012), "fullwidth" = 全角 (０１２)
digit_width = "halfwidth"
# 数字直後の 、/。 を ,/. として入力する
digit_separator_auto = true

[live_conversion]
enabled = false
debounce_ms = 80
use_llm = false
prefer_dictionary_first = true
# ライブ変換の候補数（beam 幅）: 1 = greedy（高速）, 3 = beam search（高品質、デフォルト）
beam_size = 3

[conversion]
# Space 変換のビーム幅上限（num_candidates と min をとる）。
# デフォルト 30 では num_candidates がそのまま候補数になる。
beam_size = 30

# Space 変換で表示する候補数（1〜30、デフォルト 9）。
# 新形式は [conversion].num_candidates。旧形式のルート直下 num_candidates も引き続き読める。
# num_candidates = 9

[diagnostics]
dump_active_config = false
warn_on_unknown_key = true

# 旧形式との互換用
# num_candidates = 9
"#;

const DEFAULT_KEYMAP_TEXT: &str = r#"# rakukan キーバインド設定
# 入力モード変更時に再読込されます

preset = "ms-ime-jis"
inherit_preset = true

# 追加・上書きしたいバインドだけを書いてください。
# 例:
# [[bindings]]
# key = "Ctrl+;"
# action = "ime_toggle"
"#;

#[derive(Clone, Debug)]
struct SettingsData {
    log_level: String,
    gpu_backend: Option<String>,
    n_gpu_layers: Option<u32>,
    main_gpu: i32,
    model_variant: Option<String>,
    num_candidates: Option<u32>,
    keyboard_layout: String,
    reload_on_mode_switch: bool,
    default_mode: String,
    remember_last_kana_mode: bool,
    digit_width: String,
    live_enabled: bool,
    debounce_ms: u64,
    use_llm: bool,
    prefer_dictionary_first: bool,
    beam_size: u32,
}

impl Default for SettingsData {
    fn default() -> Self {
        Self {
            log_level: "info".to_string(),
            gpu_backend: Some("auto".to_string()),
            n_gpu_layers: Some(16),
            main_gpu: 0,
            model_variant: Some("jinen-v1-xsmall-q5".to_string()),
            num_candidates: None,
            keyboard_layout: "jis".to_string(),
            reload_on_mode_switch: true,
            default_mode: "alphanumeric".to_string(),
            remember_last_kana_mode: true,
            digit_width: "halfwidth".to_string(),
            live_enabled: false,
            debounce_ms: 80,
            use_llm: false,
            prefer_dictionary_first: true,
            beam_size: 3,
        }
    }
}

struct WindowState {
    config_path: PathBuf,
    keymap_path: PathBuf,
    tab_control: HWND,
    pages: [HWND; TAB_COUNT],
    log_level: HWND,
    gpu_backend: HWND,
    n_gpu_layers: HWND,
    main_gpu: HWND,
    model_variant: HWND,
    num_candidates: HWND,
    keyboard_layout: HWND,
    reload_on_mode_switch: HWND,
    default_mode: HWND,
    remember_last_kana_mode: HWND,
    digit_width: HWND,
    live_enabled: HWND,
    debounce_ms: HWND,
    use_llm: HWND,
    prefer_dictionary_first: HWND,
    beam_size: HWND,
    keymap_preset: HWND,
    keymap_inherit_preset: HWND,
    keymap_ime_toggle: HWND,
    keymap_convert: HWND,
    keymap_commit_raw: HWND,
    keymap_cancel: HWND,
    keymap_cancel_all: HWND,
    keymap_mode_hiragana: HWND,
    keymap_mode_katakana: HWND,
    keymap_mode_alphanumeric: HWND,
}

#[derive(Clone, Debug)]
struct KeymapSettings {
    preset: String,
    inherit_preset: bool,
    ime_toggle: String,
    convert: String,
    commit_raw: String,
    cancel: String,
    cancel_all: String,
    mode_hiragana: String,
    mode_katakana: String,
    mode_alphanumeric: String,
}

impl Default for KeymapSettings {
    fn default() -> Self {
        Self::with_defaults("ms-ime-jis", true)
    }
}

impl KeymapSettings {
    fn with_defaults(preset: &str, inherit_preset: bool) -> Self {
        let mut settings = Self {
            preset: preset.to_string(),
            inherit_preset,
            ime_toggle: String::new(),
            convert: String::new(),
            commit_raw: String::new(),
            cancel: String::new(),
            cancel_all: String::new(),
            mode_hiragana: String::new(),
            mode_katakana: String::new(),
            mode_alphanumeric: String::new(),
        };
        if inherit_preset {
            for action in MANAGED_KEY_ACTIONS {
                if let Some(key) = action.default_key(preset) {
                    settings.set_binding(action, key.to_string());
                }
            }
        }
        settings
    }

    fn binding(&self, action: ManagedKeyAction) -> &str {
        match action {
            ManagedKeyAction::ImeToggle => &self.ime_toggle,
            ManagedKeyAction::Convert => &self.convert,
            ManagedKeyAction::CommitRaw => &self.commit_raw,
            ManagedKeyAction::Cancel => &self.cancel,
            ManagedKeyAction::CancelAll => &self.cancel_all,
            ManagedKeyAction::ModeHiragana => &self.mode_hiragana,
            ManagedKeyAction::ModeKatakana => &self.mode_katakana,
            ManagedKeyAction::ModeAlphanumeric => &self.mode_alphanumeric,
        }
    }

    fn set_binding(&mut self, action: ManagedKeyAction, value: String) {
        match action {
            ManagedKeyAction::ImeToggle => self.ime_toggle = value,
            ManagedKeyAction::Convert => self.convert = value,
            ManagedKeyAction::CommitRaw => self.commit_raw = value,
            ManagedKeyAction::Cancel => self.cancel = value,
            ManagedKeyAction::CancelAll => self.cancel_all = value,
            ManagedKeyAction::ModeHiragana => self.mode_hiragana = value,
            ManagedKeyAction::ModeKatakana => self.mode_katakana = value,
            ManagedKeyAction::ModeAlphanumeric => self.mode_alphanumeric = value,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ManagedKeyAction {
    ImeToggle,
    Convert,
    CommitRaw,
    Cancel,
    CancelAll,
    ModeHiragana,
    ModeKatakana,
    ModeAlphanumeric,
}

const MANAGED_KEY_ACTIONS: [ManagedKeyAction; 8] = [
    ManagedKeyAction::ImeToggle,
    ManagedKeyAction::Convert,
    ManagedKeyAction::CommitRaw,
    ManagedKeyAction::Cancel,
    ManagedKeyAction::CancelAll,
    ManagedKeyAction::ModeHiragana,
    ManagedKeyAction::ModeKatakana,
    ManagedKeyAction::ModeAlphanumeric,
];

impl ManagedKeyAction {
    fn action_name(self) -> &'static str {
        match self {
            Self::ImeToggle => "ime_toggle",
            Self::Convert => "convert",
            Self::CommitRaw => "commit_raw",
            Self::Cancel => "cancel",
            Self::CancelAll => "cancel_all",
            Self::ModeHiragana => "mode_hiragana",
            Self::ModeKatakana => "mode_katakana",
            Self::ModeAlphanumeric => "mode_alphanumeric",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::ImeToggle => "IME 切替",
            Self::Convert => "変換開始",
            Self::CommitRaw => "ひらがな確定",
            Self::Cancel => "取消",
            Self::CancelAll => "全取消",
            Self::ModeHiragana => "ひらがなモード",
            Self::ModeKatakana => "カタカナモード",
            Self::ModeAlphanumeric => "英数モード",
        }
    }

    fn default_key(self, preset: &str) -> Option<&'static str> {
        match (preset, self) {
            ("ms-ime-us", Self::ImeToggle) => Some("Ctrl+Space"),
            ("ms-ime-us", Self::Convert) => Some("Space"),
            ("ms-ime-us", Self::CommitRaw) => Some("Enter"),
            ("ms-ime-us", Self::Cancel) => Some("Escape"),
            ("ms-ime-us", Self::CancelAll) => Some("Ctrl+Backspace"),
            ("ms-ime-us", Self::ModeHiragana) => Some("Ctrl+J"),
            ("ms-ime-us", Self::ModeKatakana) => Some("Ctrl+K"),
            ("ms-ime-us", Self::ModeAlphanumeric) => Some("Ctrl+L"),
            ("ms-ime-jis", Self::ImeToggle) => Some("Zenkaku"),
            ("ms-ime-jis", Self::Convert) => Some("Space"),
            ("ms-ime-jis", Self::CommitRaw) => Some("Enter"),
            ("ms-ime-jis", Self::Cancel) => Some("Escape"),
            ("ms-ime-jis", Self::CancelAll) => Some("Ctrl+Backspace"),
            ("ms-ime-jis", Self::ModeHiragana) => Some("Hiragana_key"),
            ("ms-ime-jis", Self::ModeKatakana) => Some("Katakana"),
            ("ms-ime-jis", Self::ModeAlphanumeric) => Some("Eisuu"),
            _ => None,
        }
    }

    fn from_action_name(action: &str) -> Option<Self> {
        MANAGED_KEY_ACTIONS
            .into_iter()
            .find(|candidate| candidate.action_name() == action)
    }
}

fn main() -> Result<()> {
    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_TAB_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);

        let hinstance = GetModuleHandleW(None)?;
        let class_name = to_wide_z(WINDOW_CLASS);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let panel_class_name = to_wide_z(PANEL_CLASS);
        let panel_wc = WNDCLASSW {
            lpfnWndProc: Some(panel_wndproc),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(panel_class_name.as_ptr()),
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            ..Default::default()
        };
        RegisterClassW(&panel_wc);

        let title = to_wide_z(APP_TITLE);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_VISIBLE,
            i32::default(),
            i32::default(),
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
            None,
            None,
            hinstance,
            None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOW);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

unsafe extern "system" fn panel_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CTLCOLORSTATIC => {
            let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut _);
            let _ = SetBkMode(hdc, TRANSPARENT);
            LRESULT(GetSysColorBrush(COLOR_BTNFACE).0 as isize)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCCREATE => LRESULT(1),
        WM_CREATE => {
            if let Err(err) = init_window(hwnd) {
                show_error_message(hwnd, &format!("設定画面を初期化できませんでした。\n{err}"));
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => {
            let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut _);
            let _ = SetBkMode(hdc, TRANSPARENT);
            LRESULT(GetSysColorBrush(COLOR_BTNFACE).0 as isize)
        }
        WM_NOTIFY => {
            if let Some(state) = window_state(hwnd) {
                let header = &*(lparam.0 as *const NMHDR);
                if header.idFrom == ID_TAB_CONTROL as usize && header.code == TCN_SELCHANGE as u32 {
                    let _ = update_visible_tab(state);
                    return LRESULT(0);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as i32;
            match id {
                ID_KEYMAP_PRESET | ID_KEYMAP_INHERIT_PRESET => {
                    if let Some(state) = window_state(hwnd) {
                        if let Err(err) = apply_keymap_preset_defaults(state) {
                            show_error_message(
                                hwnd,
                                &format!("プリセットを反映できませんでした。\n{err}"),
                            );
                        }
                    }
                    LRESULT(0)
                }
                ID_KEYMAP_CLEAR_IME_TOGGLE
                | ID_KEYMAP_CLEAR_CONVERT
                | ID_KEYMAP_CLEAR_COMMIT_RAW
                | ID_KEYMAP_CLEAR_CANCEL
                | ID_KEYMAP_CLEAR_CANCEL_ALL
                | ID_KEYMAP_CLEAR_MODE_HIRAGANA
                | ID_KEYMAP_CLEAR_MODE_KATAKANA
                | ID_KEYMAP_CLEAR_MODE_ALPHANUMERIC => {
                    if let Some(state) = window_state(hwnd) {
                        clear_key_binding_field(state, id);
                    }
                    LRESULT(0)
                }
                ID_OPEN_CONFIG => {
                    if let Some(state) = window_state(hwnd) {
                        if let Err(err) = open_config_in_notepad(&state.config_path) {
                            show_error_message(
                                hwnd,
                                &format!("config.toml を開けませんでした。\n{err}"),
                            );
                        }
                    }
                    LRESULT(0)
                }
                ID_OPEN_KEYMAP => {
                    if let Some(state) = window_state(hwnd) {
                        if let Err(err) = open_keymap_in_notepad(&state.keymap_path) {
                            show_error_message(
                                hwnd,
                                &format!("keymap.toml を開けませんでした。\n{err}"),
                            );
                        }
                    }
                    LRESULT(0)
                }
                ID_SAVE => {
                    if let Some(state) = window_state(hwnd) {
                        match collect_settings(state) {
                            Ok((data, keymap)) => match save_settings(&state.config_path, &data)
                                .and_then(|_| save_keymap_settings(&state.keymap_path, &keymap))
                            {
                                Ok(()) => {
                                    show_info_message(
                                        hwnd,
                                        "設定を保存しました。\n次回の入力モード切り替え時に反映されます。",
                                    );
                                    let _ = DestroyWindow(hwnd);
                                }
                                Err(err) => {
                                    show_error_message(
                                        hwnd,
                                        &format!("設定を保存できませんでした。\n{err}"),
                                    );
                                }
                            },
                            Err(err) => show_error_message(hwnd, &err.to_string()),
                        }
                    }
                    LRESULT(0)
                }
                ID_CANCEL => {
                    let _ = DestroyWindow(hwnd);
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_NCDESTROY => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
            if !ptr.is_null() {
                let _ = Box::from_raw(ptr);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn init_window(hwnd: HWND) -> Result<()> {
    let config_path = config_path()?;
    let keymap_path = keymap_path()?;
    ensure_config_exists(&config_path)?;
    ensure_keymap_exists(&keymap_path)?;
    let settings = load_settings(&config_path)?;
    let keymap = load_keymap_settings(&keymap_path)?;
    let font = HFONT(GetStockObject(DEFAULT_GUI_FONT).0);
    let tab_x = 16;
    let tab_y = 16;
    let tab_w = 720;
    let tab_h = 700;
    let footer_y = 748;
    let page_padding = 18;

    let tab_control = create_tab_control(hwnd, ID_TAB_CONTROL, tab_x, tab_y, tab_w, tab_h, font)?;
    insert_tab_item(tab_control, 0, "基本")?;
    insert_tab_item(tab_control, 1, "入力")?;
    insert_tab_item(tab_control, 2, "キー設定")?;
    insert_tab_item(tab_control, 3, "ライブ変換")?;
    insert_tab_item(tab_control, 4, "補足")?;

    let mut page_rect = RECT::default();
    let _ = GetClientRect(tab_control, &mut page_rect);
    let _ = SendMessageW(
        tab_control,
        TCM_ADJUSTRECT,
        WPARAM(0),
        LPARAM((&mut page_rect as *mut RECT) as isize),
    );
    let page_w = page_rect.right - page_rect.left;
    let page_h = page_rect.bottom - page_rect.top;

    let page_general =
        create_page_panel(tab_control, page_rect.left, page_rect.top, page_w, page_h)?;
    let page_input = create_page_panel(tab_control, page_rect.left, page_rect.top, page_w, page_h)?;
    let page_keymap =
        create_page_panel(tab_control, page_rect.left, page_rect.top, page_w, page_h)?;
    let page_live = create_page_panel(tab_control, page_rect.left, page_rect.top, page_w, page_h)?;
    let page_misc = create_page_panel(tab_control, page_rect.left, page_rect.top, page_w, page_h)?;

    let group_x = page_padding;
    let group_w = page_w - page_padding * 2;
    let left_x = group_x + 18;
    let right_x = page_w / 2 + 6;
    let narrow_w = 260;
    let medium_w = 320;
    let full_w = group_w - 36;
    let key_label_w = 132;
    let key_field_w = 240;
    let key_clear_x = left_x + key_label_w + key_field_w + 16;

    let _ = create_group_box(
        page_general,
        "動作とモデル",
        group_x,
        14,
        group_w,
        238,
        font,
    )?;
    let log_level = create_combo(
        page_general,
        ID_LOG_LEVEL,
        "ログレベル",
        left_x,
        left_x,
        40,
        0,
        narrow_w,
        &["error", "warn", "info", "debug", "trace"],
        &settings.log_level,
        font,
    )?;
    let gpu_backend = create_combo(
        page_general,
        ID_GPU_BACKEND,
        "GPU バックエンド",
        right_x,
        right_x,
        40,
        0,
        narrow_w,
        &["auto", "cpu", "vulkan", "cuda"],
        settings.gpu_backend.as_deref().unwrap_or("auto"),
        font,
    )?;
    let n_gpu_layers = create_labeled_edit(
        page_general,
        ID_N_GPU_LAYERS,
        "GPU レイヤー数",
        left_x,
        left_x,
        86,
        0,
        narrow_w,
        settings
            .n_gpu_layers
            .map(|v| v.to_string())
            .unwrap_or_default()
            .as_str(),
        font,
    )?;
    let main_gpu = create_labeled_edit(
        page_general,
        ID_MAIN_GPU,
        "使用 GPU インデックス",
        right_x,
        right_x,
        86,
        0,
        narrow_w,
        &settings.main_gpu.to_string(),
        font,
    )?;
    let model_variant = create_labeled_edit(
        page_general,
        ID_MODEL_VARIANT,
        "モデル ID",
        left_x,
        left_x,
        132,
        0,
        full_w,
        settings.model_variant.as_deref().unwrap_or(""),
        font,
    )?;
    let num_candidates = create_labeled_edit(
        page_general,
        ID_NUM_CANDIDATES,
        "候補数 (1-30)",
        left_x,
        left_x,
        178,
        0,
        narrow_w,
        settings
            .num_candidates
            .map(|v| v.to_string())
            .unwrap_or_default()
            .as_str(),
        font,
    )?;

    let _ = create_group_box(page_input, "キーボード", group_x, 14, group_w, 118, font)?;
    let keyboard_layout = create_combo(
        page_input,
        ID_KEYBOARD_LAYOUT,
        "キーボード配列",
        left_x,
        left_x,
        40,
        0,
        narrow_w,
        &["jis", "us", "custom"],
        &settings.keyboard_layout,
        font,
    )?;
    let reload_on_mode_switch = create_checkbox(
        page_input,
        ID_RELOAD_ON_MODE_SWITCH,
        "入力モード切替時に config.toml を再読込する",
        left_x,
        86,
        full_w,
        24,
        settings.reload_on_mode_switch,
        font,
    )?;

    let _ = create_group_box(page_input, "入力モード", group_x, 148, group_w, 174, font)?;
    let default_mode = create_combo(
        page_input,
        ID_DEFAULT_MODE,
        "初期入力モード",
        left_x,
        left_x,
        174,
        0,
        narrow_w,
        &["alphanumeric", "hiragana"],
        &settings.default_mode,
        font,
    )?;
    let digit_width = create_combo(
        page_input,
        ID_DIGIT_WIDTH,
        "数字の入力幅",
        right_x,
        right_x,
        174,
        0,
        narrow_w,
        &["halfwidth", "fullwidth"],
        &settings.digit_width,
        font,
    )?;
    let remember_last_kana_mode = create_checkbox(
        page_input,
        ID_REMEMBER_LAST_KANA_MODE,
        "前回のかなモードをアプリごとに記憶する",
        left_x,
        226,
        full_w,
        24,
        settings.remember_last_kana_mode,
        font,
    )?;

    let _ = create_group_box(page_keymap, "プリセット", group_x, 14, group_w, 114, font)?;
    let keymap_preset = create_combo(
        page_keymap,
        ID_KEYMAP_PRESET,
        "キーバインドプリセット",
        left_x,
        left_x,
        40,
        0,
        medium_w,
        &["ms-ime-jis", "ms-ime-us", "custom"],
        &keymap.preset,
        font,
    )?;
    let keymap_inherit_preset = create_checkbox(
        page_keymap,
        ID_KEYMAP_INHERIT_PRESET,
        "プリセットをベースに主要ショートカットを上書きする",
        left_x,
        86,
        full_w,
        24,
        keymap.inherit_preset,
        font,
    )?;

    let _ = create_group_box(
        page_keymap,
        "主要ショートカット",
        group_x,
        144,
        group_w,
        340,
        font,
    )?;
    let mut key_y = 170;
    let keymap_ime_toggle = create_key_capture_row(
        page_keymap,
        ID_KEYMAP_IME_TOGGLE,
        ID_KEYMAP_CLEAR_IME_TOGGLE,
        ManagedKeyAction::ImeToggle.label(),
        left_x,
        key_y,
        key_label_w,
        key_field_w,
        key_clear_x,
        &keymap.ime_toggle,
        font,
    )?;
    key_y += 34;
    let keymap_convert = create_key_capture_row(
        page_keymap,
        ID_KEYMAP_CONVERT,
        ID_KEYMAP_CLEAR_CONVERT,
        ManagedKeyAction::Convert.label(),
        left_x,
        key_y,
        key_label_w,
        key_field_w,
        key_clear_x,
        &keymap.convert,
        font,
    )?;
    key_y += 34;
    let keymap_commit_raw = create_key_capture_row(
        page_keymap,
        ID_KEYMAP_COMMIT_RAW,
        ID_KEYMAP_CLEAR_COMMIT_RAW,
        ManagedKeyAction::CommitRaw.label(),
        left_x,
        key_y,
        key_label_w,
        key_field_w,
        key_clear_x,
        &keymap.commit_raw,
        font,
    )?;
    key_y += 34;
    let keymap_cancel = create_key_capture_row(
        page_keymap,
        ID_KEYMAP_CANCEL,
        ID_KEYMAP_CLEAR_CANCEL,
        ManagedKeyAction::Cancel.label(),
        left_x,
        key_y,
        key_label_w,
        key_field_w,
        key_clear_x,
        &keymap.cancel,
        font,
    )?;
    key_y += 34;
    let keymap_cancel_all = create_key_capture_row(
        page_keymap,
        ID_KEYMAP_CANCEL_ALL,
        ID_KEYMAP_CLEAR_CANCEL_ALL,
        ManagedKeyAction::CancelAll.label(),
        left_x,
        key_y,
        key_label_w,
        key_field_w,
        key_clear_x,
        &keymap.cancel_all,
        font,
    )?;
    key_y += 34;
    let keymap_mode_hiragana = create_key_capture_row(
        page_keymap,
        ID_KEYMAP_MODE_HIRAGANA,
        ID_KEYMAP_CLEAR_MODE_HIRAGANA,
        ManagedKeyAction::ModeHiragana.label(),
        left_x,
        key_y,
        key_label_w,
        key_field_w,
        key_clear_x,
        &keymap.mode_hiragana,
        font,
    )?;
    key_y += 34;
    let keymap_mode_katakana = create_key_capture_row(
        page_keymap,
        ID_KEYMAP_MODE_KATAKANA,
        ID_KEYMAP_CLEAR_MODE_KATAKANA,
        ManagedKeyAction::ModeKatakana.label(),
        left_x,
        key_y,
        key_label_w,
        key_field_w,
        key_clear_x,
        &keymap.mode_katakana,
        font,
    )?;
    key_y += 34;
    let keymap_mode_alphanumeric = create_key_capture_row(
        page_keymap,
        ID_KEYMAP_MODE_ALPHANUMERIC,
        ID_KEYMAP_CLEAR_MODE_ALPHANUMERIC,
        ManagedKeyAction::ModeAlphanumeric.label(),
        left_x,
        key_y,
        key_label_w,
        key_field_w,
        key_clear_x,
        &keymap.mode_alphanumeric,
        font,
    )?;
    let _ = create_static(
        page_keymap,
        "対応するキー名を直接入力してください。例: Ctrl+Space, Henkan, Zenkaku。複数割り当ては keymap.toml で調整できます。",
        left_x,
        450,
        full_w,
        20,
        font,
    )?;

    let _ = create_group_box(page_keymap, "詳細編集", group_x, 500, group_w, 88, font)?;
    let _ = create_static(
        page_keymap,
        "複数キーや未表示のアクションは keymap.toml を直接編集できます。",
        left_x,
        526,
        full_w,
        20,
        font,
    )?;
    let _ = create_button(
        page_keymap,
        ID_OPEN_KEYMAP,
        "keymap.toml を開く",
        left_x,
        550,
        180,
        30,
        font,
        false,
    )?;

    let _ = create_group_box(page_live, "ライブ変換", group_x, 14, group_w, 214, font)?;
    let live_enabled = create_checkbox(
        page_live,
        ID_LIVE_ENABLED,
        "ライブ変換を有効にする",
        left_x,
        40,
        full_w,
        24,
        settings.live_enabled,
        font,
    )?;
    let debounce_ms = create_labeled_edit(
        page_live,
        ID_DEBOUNCE_MS,
        "デバウンス (ms)",
        left_x,
        left_x,
        84,
        0,
        medium_w,
        &settings.debounce_ms.to_string(),
        font,
    )?;
    let beam_size = create_labeled_edit(
        page_live,
        ID_BEAM_SIZE,
        "beam_size (1-9)",
        right_x,
        right_x,
        84,
        0,
        medium_w,
        &settings.beam_size.to_string(),
        font,
    )?;
    let use_llm = create_checkbox(
        page_live,
        ID_USE_LLM,
        "LLM をライブ変換に使う",
        left_x,
        136,
        full_w,
        24,
        settings.use_llm,
        font,
    )?;
    let prefer_dictionary_first = create_checkbox(
        page_live,
        ID_PREFER_DICTIONARY_FIRST,
        "辞書候補を優先してから LLM 候補を使う",
        left_x,
        170,
        full_w,
        24,
        settings.prefer_dictionary_first,
        font,
    )?;

    let _ = create_group_box(page_misc, "ファイル", group_x, 14, group_w, 176, font)?;
    let _ = create_static(
        page_misc,
        "詳細設定や未対応項目は config.toml / keymap.toml を直接編集できます。",
        left_x,
        40,
        full_w,
        20,
        font,
    )?;
    let _ = create_static(
        page_misc,
        "保存した設定は次回の入力モード切り替え時に反映されます。",
        left_x,
        68,
        full_w,
        20,
        font,
    )?;
    let _ = create_button(
        page_misc,
        ID_OPEN_CONFIG,
        "config.toml を開く",
        left_x,
        104,
        180,
        30,
        font,
        false,
    )?;

    let _ = create_button(hwnd, ID_SAVE, "保存", 522, footer_y, 100, 32, font, true)?;
    let _ = create_button(
        hwnd,
        ID_CANCEL,
        "キャンセル",
        632,
        footer_y,
        100,
        32,
        font,
        false,
    )?;

    let state = Box::new(WindowState {
        config_path,
        keymap_path,
        tab_control,
        pages: [page_general, page_input, page_keymap, page_live, page_misc],
        log_level,
        gpu_backend,
        n_gpu_layers,
        main_gpu,
        model_variant,
        num_candidates,
        keyboard_layout,
        reload_on_mode_switch,
        default_mode,
        remember_last_kana_mode,
        digit_width,
        live_enabled,
        debounce_ms,
        use_llm,
        prefer_dictionary_first,
        beam_size,
        keymap_preset,
        keymap_inherit_preset,
        keymap_ime_toggle,
        keymap_convert,
        keymap_commit_raw,
        keymap_cancel,
        keymap_cancel_all,
        keymap_mode_hiragana,
        keymap_mode_katakana,
        keymap_mode_alphanumeric,
    });
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
    if let Some(state) = window_state(hwnd) {
        update_visible_tab(state)?;
    }
    Ok(())
}

unsafe fn create_tab_control(
    hwnd: HWND,
    id: i32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    font: HFONT,
) -> Result<HWND> {
    let tab = create_control(
        hwnd,
        w!("SysTabControl32"),
        "",
        x,
        y,
        width,
        height,
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0),
        font,
        id,
    )?;
    Ok(tab)
}

unsafe fn insert_tab_item(tab: HWND, index: usize, title: &str) -> Result<()> {
    let mut wide = to_wide_z(title);
    let item = TCITEMW {
        mask: TCIF_TEXT,
        pszText: windows::core::PWSTR(wide.as_mut_ptr()),
        ..Default::default()
    };
    let result = SendMessageW(
        tab,
        TCM_INSERTITEMW,
        WPARAM(index),
        LPARAM((&item as *const TCITEMW) as isize),
    )
    .0;
    if result < 0 {
        bail!("タブ項目を追加できませんでした。");
    }
    Ok(())
}

unsafe fn create_page_panel(parent: HWND, x: i32, y: i32, width: i32, height: i32) -> Result<HWND> {
    let panel_class = to_wide_z(PANEL_CLASS);
    CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        PCWSTR(panel_class.as_ptr()),
        PCWSTR::null(),
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
        x,
        y,
        width,
        height,
        parent,
        None,
        GetModuleHandleW(None)?,
        None,
    )
    .map_err(Into::into)
}

unsafe fn create_key_capture_row(
    hwnd: HWND,
    id: i32,
    clear_id: i32,
    label: &str,
    x: i32,
    y: i32,
    label_w: i32,
    field_w: i32,
    clear_x: i32,
    value: &str,
    font: HFONT,
) -> Result<HWND> {
    let _ = create_static(hwnd, label, x, y + 4, label_w, 20, font)?;
    let field = create_control(
        hwnd,
        w!("EDIT"),
        value,
        x + label_w,
        y,
        field_w,
        24,
        WINDOW_STYLE(
            WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | WS_BORDER.0 | ES_AUTOHSCROLL as u32,
        ),
        font,
        id,
    )?;
    let _ = create_button(hwnd, clear_id, "解除", clear_x, y - 1, 64, 26, font, false)?;
    Ok(field)
}

unsafe fn update_visible_tab(state: &WindowState) -> Result<()> {
    let current = SendMessageW(state.tab_control, TCM_GETCURSEL, WPARAM(0), LPARAM(0)).0;
    if current < 0 {
        bail!("現在のタブを取得できませんでした。");
    }
    for (index, page) in state.pages.iter().enumerate() {
        let _ = ShowWindow(
            *page,
            if index == current as usize {
                SW_SHOW
            } else {
                SW_HIDE
            },
        );
    }
    Ok(())
}

#[allow(dead_code)]
unsafe fn create_section_label(
    hwnd: HWND,
    text: &str,
    x: i32,
    y: i32,
    font: HFONT,
) -> Result<HWND> {
    create_control(
        hwnd,
        w!("STATIC"),
        text,
        x,
        y,
        220,
        24,
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
        font,
        0,
    )
}

unsafe fn create_group_box(
    hwnd: HWND,
    text: &str,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    font: HFONT,
) -> Result<HWND> {
    create_control(
        hwnd,
        w!("BUTTON"),
        text,
        x,
        y,
        width,
        height,
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | BS_GROUPBOX as u32),
        font,
        0,
    )
}

unsafe fn create_static(
    hwnd: HWND,
    text: &str,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    font: HFONT,
) -> Result<HWND> {
    create_control(
        hwnd,
        w!("STATIC"),
        text,
        x,
        y,
        width,
        height,
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
        font,
        0,
    )
}

unsafe fn create_labeled_edit(
    hwnd: HWND,
    id: i32,
    label: &str,
    label_x: i32,
    field_x: i32,
    y: i32,
    label_w: i32,
    field_w: i32,
    value: &str,
    font: HFONT,
) -> Result<HWND> {
    if label_w > 0 {
        let _ = create_static(hwnd, label, label_x, y + 4, label_w, 20, font)?;
        return create_control(
            hwnd,
            w!("EDIT"),
            value,
            field_x,
            y,
            field_w,
            24,
            WINDOW_STYLE(
                WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | WS_BORDER.0 | ES_AUTOHSCROLL as u32,
            ),
            font,
            id,
        );
    }

    let _ = create_static(hwnd, label, label_x, y, field_w, 18, font)?;
    create_control(
        hwnd,
        w!("EDIT"),
        value,
        field_x,
        y + 20,
        field_w,
        24,
        WINDOW_STYLE(
            WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | WS_BORDER.0 | ES_AUTOHSCROLL as u32,
        ),
        font,
        id,
    )
}

unsafe fn create_combo(
    hwnd: HWND,
    id: i32,
    label: &str,
    label_x: i32,
    field_x: i32,
    y: i32,
    label_w: i32,
    field_w: i32,
    options: &[&str],
    selected: &str,
    font: HFONT,
) -> Result<HWND> {
    let control_y = if label_w > 0 {
        let _ = create_static(hwnd, label, label_x, y + 4, label_w, 20, font)?;
        y
    } else {
        let _ = create_static(hwnd, label, label_x, y, field_w, 18, font)?;
        y + 20
    };
    let combo = create_control(
        hwnd,
        w!("COMBOBOX"),
        "",
        field_x,
        control_y,
        field_w,
        200,
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | CBS_DROPDOWNLIST as u32),
        font,
        id,
    )?;
    for option in options {
        let wide = to_wide_z(option);
        let _ = SendMessageW(
            combo,
            CB_ADDSTRING,
            WPARAM(0),
            LPARAM(wide.as_ptr() as isize),
        );
    }
    let selected_index = options
        .iter()
        .position(|option| option.eq_ignore_ascii_case(selected))
        .unwrap_or(0);
    let _ = SendMessageW(combo, CB_SETCURSEL, WPARAM(selected_index), LPARAM(0));
    Ok(combo)
}

unsafe fn create_checkbox(
    hwnd: HWND,
    id: i32,
    text: &str,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    checked: bool,
    font: HFONT,
) -> Result<HWND> {
    let checkbox = create_control(
        hwnd,
        w!("BUTTON"),
        text,
        x,
        y,
        width,
        height,
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32),
        font,
        id,
    )?;
    let _ = SendMessageW(
        checkbox,
        BM_SETCHECK,
        WPARAM(if checked { 1 } else { 0 }),
        LPARAM(0),
    );
    Ok(checkbox)
}

unsafe fn create_button(
    hwnd: HWND,
    id: i32,
    text: &str,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    font: HFONT,
    is_default: bool,
) -> Result<HWND> {
    let mut style = WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0;
    if is_default {
        style |= BS_DEFPUSHBUTTON as u32;
    }
    create_control(
        hwnd,
        w!("BUTTON"),
        text,
        x,
        y,
        width,
        height,
        WINDOW_STYLE(style),
        font,
        id,
    )
}

unsafe fn create_control(
    hwnd: HWND,
    class: PCWSTR,
    text: &str,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    style: WINDOW_STYLE,
    font: HFONT,
    id: i32,
) -> Result<HWND> {
    let wide = to_wide_z(text);
    let ex_style = if class == w!("EDIT") {
        WS_EX_CLIENTEDGE
    } else {
        WINDOW_EX_STYLE::default()
    };
    let control = CreateWindowExW(
        ex_style,
        class,
        PCWSTR(wide.as_ptr()),
        style,
        x,
        y,
        width,
        height,
        hwnd,
        HMENU(id as isize as *mut _),
        GetModuleHandleW(None)?,
        None,
    )?;
    let _ = SendMessageW(control, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
    Ok(control)
}

fn collect_settings(state: &WindowState) -> Result<(SettingsData, KeymapSettings)> {
    let log_level = combo_text(state.log_level)?;
    let gpu_backend = Some(combo_text(state.gpu_backend)?);
    let n_gpu_layers = parse_optional_u32("GPU レイヤー数", &window_text(state.n_gpu_layers)?)?;
    let main_gpu = parse_i32("使用 GPU インデックス", &window_text(state.main_gpu)?)?;
    let model_variant = optional_string(window_text(state.model_variant)?);
    let num_candidates = parse_optional_u32("候補数", &window_text(state.num_candidates)?)?;
    if let Some(value) = num_candidates {
        if !(1..=30).contains(&value) {
            bail!("候補数は 1 から 30 の範囲で入力してください。");
        }
    }
    let keyboard_layout = combo_text(state.keyboard_layout)?;
    let default_mode = combo_text(state.default_mode)?;
    let digit_width = combo_text(state.digit_width)?;
    let live_enabled = checkbox_checked(state.live_enabled);
    let debounce_ms = parse_u64("デバウンス", &window_text(state.debounce_ms)?)?;
    let beam_size = parse_u32("beam_size", &window_text(state.beam_size)?)?;
    if !(1..=9).contains(&beam_size) {
        bail!("beam_size は 1 から 9 の範囲で入力してください。");
    }

    Ok((
        SettingsData {
            log_level,
            gpu_backend,
            n_gpu_layers,
            main_gpu,
            model_variant,
            num_candidates,
            keyboard_layout,
            reload_on_mode_switch: checkbox_checked(state.reload_on_mode_switch),
            default_mode,
            remember_last_kana_mode: checkbox_checked(state.remember_last_kana_mode),
            digit_width,
            live_enabled,
            debounce_ms,
            use_llm: checkbox_checked(state.use_llm),
            prefer_dictionary_first: checkbox_checked(state.prefer_dictionary_first),
            beam_size,
        },
        KeymapSettings {
            preset: combo_text(state.keymap_preset)?,
            inherit_preset: checkbox_checked(state.keymap_inherit_preset),
            ime_toggle: validate_key_binding(
                ManagedKeyAction::ImeToggle.label(),
                &window_text(state.keymap_ime_toggle)?,
            )?,
            convert: validate_key_binding(
                ManagedKeyAction::Convert.label(),
                &window_text(state.keymap_convert)?,
            )?,
            commit_raw: validate_key_binding(
                ManagedKeyAction::CommitRaw.label(),
                &window_text(state.keymap_commit_raw)?,
            )?,
            cancel: validate_key_binding(
                ManagedKeyAction::Cancel.label(),
                &window_text(state.keymap_cancel)?,
            )?,
            cancel_all: validate_key_binding(
                ManagedKeyAction::CancelAll.label(),
                &window_text(state.keymap_cancel_all)?,
            )?,
            mode_hiragana: validate_key_binding(
                ManagedKeyAction::ModeHiragana.label(),
                &window_text(state.keymap_mode_hiragana)?,
            )?,
            mode_katakana: validate_key_binding(
                ManagedKeyAction::ModeKatakana.label(),
                &window_text(state.keymap_mode_katakana)?,
            )?,
            mode_alphanumeric: validate_key_binding(
                ManagedKeyAction::ModeAlphanumeric.label(),
                &window_text(state.keymap_mode_alphanumeric)?,
            )?,
        },
    ))
}

fn load_settings(path: &Path) -> Result<SettingsData> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let value: Value = text
        .parse::<Value>()
        .with_context(|| format!("parse {}", path.display()))?;

    Ok(SettingsData {
        log_level: get_string(&value, &["general", "log_level"])
            .unwrap_or_else(|| "debug".to_string()),
        gpu_backend: get_string(&value, &["general", "gpu_backend"]),
        n_gpu_layers: get_u32(&value, &["general", "n_gpu_layers"]),
        main_gpu: get_i32(&value, &["general", "main_gpu"]).unwrap_or(0),
        model_variant: get_string(&value, &["general", "model_variant"]),
        num_candidates: get_u32(&value, &["conversion", "num_candidates"])
            .or_else(|| get_u32(&value, &["num_candidates"])),
        keyboard_layout: get_string(&value, &["keyboard", "layout"])
            .unwrap_or_else(|| "jis".to_string()),
        reload_on_mode_switch: get_bool(&value, &["keyboard", "reload_on_mode_switch"])
            .unwrap_or(true),
        default_mode: get_string(&value, &["input", "default_mode"])
            .unwrap_or_else(|| "alphanumeric".to_string()),
        remember_last_kana_mode: get_bool(&value, &["input", "remember_last_kana_mode"])
            .unwrap_or(true),
        digit_width: get_string(&value, &["input", "digit_width"])
            .unwrap_or_else(|| "halfwidth".to_string()),
        live_enabled: get_bool(&value, &["live_conversion", "enabled"]).unwrap_or(false),
        debounce_ms: get_u64(&value, &["live_conversion", "debounce_ms"]).unwrap_or(80),
        use_llm: get_bool(&value, &["live_conversion", "use_llm"]).unwrap_or(false),
        prefer_dictionary_first: get_bool(&value, &["live_conversion", "prefer_dictionary_first"])
            .unwrap_or(true),
        beam_size: get_u32(&value, &["live_conversion", "beam_size"]).unwrap_or(3),
    })
}

fn save_settings(path: &Path, settings: &SettingsData) -> Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|_| DEFAULT_CONFIG_TEXT.to_string());
    let mut value = text
        .parse::<Value>()
        .unwrap_or_else(|_| Value::Table(Map::new()));

    set_string(
        &mut value,
        &["general", "log_level"],
        settings.log_level.clone(),
    );
    set_optional_string(
        &mut value,
        &["general", "gpu_backend"],
        settings.gpu_backend.clone(),
    );
    set_optional_u32(
        &mut value,
        &["general", "n_gpu_layers"],
        settings.n_gpu_layers,
    );
    set_i32(&mut value, &["general", "main_gpu"], settings.main_gpu);
    set_optional_string(
        &mut value,
        &["general", "model_variant"],
        settings.model_variant.clone(),
    );
    set_string(
        &mut value,
        &["keyboard", "layout"],
        settings.keyboard_layout.clone(),
    );
    set_bool(
        &mut value,
        &["keyboard", "reload_on_mode_switch"],
        settings.reload_on_mode_switch,
    );
    set_string(
        &mut value,
        &["input", "default_mode"],
        settings.default_mode.clone(),
    );
    set_bool(
        &mut value,
        &["input", "remember_last_kana_mode"],
        settings.remember_last_kana_mode,
    );
    set_string(
        &mut value,
        &["input", "digit_width"],
        settings.digit_width.clone(),
    );
    set_bool(
        &mut value,
        &["live_conversion", "enabled"],
        settings.live_enabled,
    );
    set_u64(
        &mut value,
        &["live_conversion", "debounce_ms"],
        settings.debounce_ms,
    );
    set_bool(
        &mut value,
        &["live_conversion", "use_llm"],
        settings.use_llm,
    );
    set_bool(
        &mut value,
        &["live_conversion", "prefer_dictionary_first"],
        settings.prefer_dictionary_first,
    );
    set_u32(
        &mut value,
        &["live_conversion", "beam_size"],
        settings.beam_size,
    );
    set_optional_u32(
        &mut value,
        &["conversion", "num_candidates"],
        settings.num_candidates,
    );
    remove_value(&mut value, &["num_candidates"]);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, toml::to_string_pretty(&value)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn load_keymap_settings(path: &Path) -> Result<KeymapSettings> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let value: Value = text
        .parse::<Value>()
        .with_context(|| format!("parse {}", path.display()))?;
    Ok(load_keymap_settings_from_value(&value))
}

fn save_keymap_settings(path: &Path, settings: &KeymapSettings) -> Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|_| DEFAULT_KEYMAP_TEXT.to_string());
    let mut value = text
        .parse::<Value>()
        .unwrap_or_else(|_| Value::Table(Map::new()));
    set_string(&mut value, &["preset"], settings.preset.clone());
    set_bool(&mut value, &["inherit_preset"], settings.inherit_preset);
    update_managed_bindings(&mut value, settings);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, toml::to_string_pretty(&value)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn config_path() -> Result<PathBuf> {
    let appdata = std::env::var("APPDATA").map_err(|_| anyhow!("APPDATA not set"))?;
    Ok(PathBuf::from(appdata).join("rakukan").join("config.toml"))
}

fn keymap_path() -> Result<PathBuf> {
    let appdata = std::env::var("APPDATA").map_err(|_| anyhow!("APPDATA not set"))?;
    Ok(PathBuf::from(appdata).join("rakukan").join("keymap.toml"))
}

fn ensure_config_exists(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, DEFAULT_CONFIG_TEXT)?;
    Ok(())
}

fn ensure_keymap_exists(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, DEFAULT_KEYMAP_TEXT)?;
    Ok(())
}

fn open_config_in_notepad(path: &Path) -> Result<()> {
    std::process::Command::new("notepad.exe")
        .arg(path)
        .spawn()
        .with_context(|| format!("spawn notepad for {}", path.display()))?;
    Ok(())
}

fn open_keymap_in_notepad(path: &Path) -> Result<()> {
    std::process::Command::new("notepad.exe")
        .arg(path)
        .spawn()
        .with_context(|| format!("spawn notepad for {}", path.display()))?;
    Ok(())
}

fn load_keymap_settings_from_value(value: &Value) -> KeymapSettings {
    let preset = get_string(value, &["preset"]).unwrap_or_else(|| "ms-ime-jis".to_string());
    let inherit_preset = get_bool(value, &["inherit_preset"]).unwrap_or(true);
    let mut settings = KeymapSettings::with_defaults(&preset, inherit_preset);
    let mut seen_actions = HashSet::new();
    if let Some(bindings) = value.get("bindings").and_then(Value::as_array) {
        for binding in bindings {
            let Some((key, action)) = binding_entry(binding) else {
                continue;
            };
            let Some(action) = ManagedKeyAction::from_action_name(&action) else {
                continue;
            };
            if seen_actions.insert(action) {
                settings.set_binding(action, key);
            }
        }
    }
    settings
}

fn update_managed_bindings(root: &mut Value, settings: &KeymapSettings) {
    let existing = root
        .get("bindings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut preserved = Vec::new();
    let mut extras: HashMap<&'static str, Vec<String>> = HashMap::new();

    for binding in existing {
        match binding_entry(&binding) {
            Some((key, action)) => {
                if let Some(managed) = ManagedKeyAction::from_action_name(&action) {
                    extras.entry(managed.action_name()).or_default().push(key);
                } else {
                    preserved.push(binding);
                }
            }
            None => preserved.push(binding),
        }
    }

    let mut bindings = preserved;
    for action in MANAGED_KEY_ACTIONS {
        let primary = settings.binding(action).trim();
        if primary.is_empty() {
            continue;
        }
        bindings.push(make_binding_value(primary, action.action_name()));
        if let Some(existing_keys) = extras.remove(action.action_name()) {
            for key in existing_keys {
                if key != primary {
                    bindings.push(make_binding_value(&key, action.action_name()));
                }
            }
        }
    }
    set_value(root, &["bindings"], Value::Array(bindings));
}

fn binding_entry(value: &Value) -> Option<(String, String)> {
    let table = value.as_table()?;
    let key = table.get("key")?.as_str()?.trim();
    let action = table.get("action")?.as_str()?.trim();
    Some((key.to_string(), action.to_string()))
}

fn make_binding_value(key: &str, action: &str) -> Value {
    let mut table = Map::new();
    table.insert("key".to_string(), Value::String(key.to_string()));
    table.insert("action".to_string(), Value::String(action.to_string()));
    Value::Table(table)
}

fn get_string(root: &Value, path: &[&str]) -> Option<String> {
    get_value(root, path)?
        .as_str()
        .map(|value| value.to_string())
}

fn get_bool(root: &Value, path: &[&str]) -> Option<bool> {
    get_value(root, path)?.as_bool()
}

fn get_u32(root: &Value, path: &[&str]) -> Option<u32> {
    get_value(root, path)?
        .as_integer()
        .and_then(|value| value.try_into().ok())
}

fn get_u64(root: &Value, path: &[&str]) -> Option<u64> {
    get_value(root, path)?
        .as_integer()
        .and_then(|value| value.try_into().ok())
}

fn get_i32(root: &Value, path: &[&str]) -> Option<i32> {
    get_value(root, path)?
        .as_integer()
        .and_then(|value| value.try_into().ok())
}

fn get_value<'a>(root: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = root;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

fn set_string(root: &mut Value, path: &[&str], value: String) {
    set_value(root, path, Value::String(value));
}

fn set_bool(root: &mut Value, path: &[&str], value: bool) {
    set_value(root, path, Value::Boolean(value));
}

fn set_u32(root: &mut Value, path: &[&str], value: u32) {
    set_value(root, path, Value::Integer(i64::from(value)));
}

fn set_u64(root: &mut Value, path: &[&str], value: u64) {
    set_value(root, path, Value::Integer(value as i64));
}

fn set_i32(root: &mut Value, path: &[&str], value: i32) {
    set_value(root, path, Value::Integer(i64::from(value)));
}

fn set_optional_string(root: &mut Value, path: &[&str], value: Option<String>) {
    match value {
        Some(value) => set_string(root, path, value),
        None => remove_value(root, path),
    }
}

fn set_optional_u32(root: &mut Value, path: &[&str], value: Option<u32>) {
    match value {
        Some(value) => set_u32(root, path, value),
        None => remove_value(root, path),
    }
}

fn set_value(root: &mut Value, path: &[&str], value: Value) {
    if path.is_empty() {
        *root = value;
        return;
    }

    let mut current = ensure_table(root);
    for key in &path[..path.len() - 1] {
        let entry = current
            .entry((*key).to_string())
            .or_insert_with(|| Value::Table(Map::new()));
        current = ensure_table(entry);
    }
    current.insert(path[path.len() - 1].to_string(), value);
}

fn remove_value(root: &mut Value, path: &[&str]) {
    if path.is_empty() {
        return;
    }
    let mut current = match root.as_table_mut() {
        Some(table) => table,
        None => return,
    };
    for key in &path[..path.len() - 1] {
        let Some(next) = current.get_mut(*key) else {
            return;
        };
        let Some(next_table) = next.as_table_mut() else {
            return;
        };
        current = next_table;
    }
    current.remove(path[path.len() - 1]);
}

fn ensure_table(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_table() {
        *value = Value::Table(Map::new());
    }
    value.as_table_mut().expect("table")
}

fn combo_text(hwnd: HWND) -> Result<String> {
    let index = unsafe { SendMessageW(hwnd, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 };
    if index < 0 {
        bail!("コンボボックスの選択値を取得できませんでした。");
    }
    window_text(hwnd)
}

fn checkbox_checked(hwnd: HWND) -> bool {
    unsafe { SendMessageW(hwnd, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == 1 }
}

fn window_text(hwnd: HWND) -> Result<String> {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        let mut buf = vec![0u16; len as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut buf);
        Ok(String::from_utf16_lossy(&buf[..copied as usize]))
    }
}

fn optional_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn validate_key_binding(label: &str, value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if is_valid_key_binding(trimmed) {
        Ok(trimmed.to_string())
    } else {
        bail!(
            "{label} は対応しているキー名で入力してください。例: Ctrl+Space, Henkan, Zenkaku, F6"
        );
    }
}

fn is_valid_key_binding(value: &str) -> bool {
    let mut saw_key = false;
    for part in value.split('+') {
        let token = part.trim().to_ascii_lowercase();
        if token.is_empty() {
            return false;
        }
        match token.as_str() {
            "ctrl" | "control" | "shift" | "alt" => {}
            _ if !saw_key && is_supported_key_name(&token) => saw_key = true,
            _ => return false,
        }
    }
    saw_key
}

fn is_supported_key_name(name: &str) -> bool {
    matches!(
        name,
        "backspace"
            | "bs"
            | "tab"
            | "enter"
            | "return"
            | "escape"
            | "esc"
            | "space"
            | "backquote"
            | "grave"
            | "semicolon"
            | "equal"
            | "comma"
            | "minus"
            | "period"
            | "slash"
            | "leftbracket"
            | "backslash"
            | "rightbracket"
            | "quote"
            | "pageup"
            | "pgup"
            | "pagedown"
            | "pgdn"
            | "end"
            | "home"
            | "left"
            | "up"
            | "right"
            | "down"
            | "delete"
            | "del"
            | "f1"
            | "f2"
            | "f3"
            | "f4"
            | "f5"
            | "f6"
            | "f7"
            | "f8"
            | "f9"
            | "f10"
            | "f11"
            | "f12"
            | "zenkaku"
            | "hankaku"
            | "kanji"
            | "henkan"
            | "muhenkan"
            | "eisuu"
            | "alphanumeric"
            | "katakana"
            | "hiragana_key"
            | "caps"
    ) || (name.len() == 1 && name.chars().all(|c| c.is_ascii_alphabetic()))
}

fn parse_optional_u32(label: &str, value: &str) -> Result<Option<u32>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.parse::<u32>().with_context(|| {
            format!("{label} は数値で入力してください。")
        })?))
    }
}

fn parse_u32(label: &str, value: &str) -> Result<u32> {
    value
        .trim()
        .parse::<u32>()
        .with_context(|| format!("{label} は数値で入力してください。"))
}

fn parse_u64(label: &str, value: &str) -> Result<u64> {
    value
        .trim()
        .parse::<u64>()
        .with_context(|| format!("{label} は数値で入力してください。"))
}

fn parse_i32(label: &str, value: &str) -> Result<i32> {
    value
        .trim()
        .parse::<i32>()
        .with_context(|| format!("{label} は整数で入力してください。"))
}

fn to_wide_z(text: &str) -> Vec<u16> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    wide
}

unsafe fn window_state<'a>(hwnd: HWND) -> Option<&'a mut WindowState> {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
    ptr.as_mut()
}

unsafe fn apply_keymap_preset_defaults(state: &WindowState) -> Result<()> {
    if !checkbox_checked(state.keymap_inherit_preset) {
        return Ok(());
    }
    let preset = combo_text(state.keymap_preset)?;
    let defaults = KeymapSettings::with_defaults(&preset, true);
    set_control_text(state.keymap_ime_toggle, defaults.ime_toggle.as_str());
    set_control_text(state.keymap_convert, defaults.convert.as_str());
    set_control_text(state.keymap_commit_raw, defaults.commit_raw.as_str());
    set_control_text(state.keymap_cancel, defaults.cancel.as_str());
    set_control_text(state.keymap_cancel_all, defaults.cancel_all.as_str());
    set_control_text(state.keymap_mode_hiragana, defaults.mode_hiragana.as_str());
    set_control_text(state.keymap_mode_katakana, defaults.mode_katakana.as_str());
    set_control_text(
        state.keymap_mode_alphanumeric,
        defaults.mode_alphanumeric.as_str(),
    );
    Ok(())
}

unsafe fn set_control_text(hwnd: HWND, text: &str) {
    let wide = to_wide_z(text);
    let _ = SetWindowTextW(hwnd, PCWSTR(wide.as_ptr()));
}

unsafe fn clear_key_binding_field(state: &WindowState, id: i32) {
    let target = match id {
        ID_KEYMAP_CLEAR_IME_TOGGLE => state.keymap_ime_toggle,
        ID_KEYMAP_CLEAR_CONVERT => state.keymap_convert,
        ID_KEYMAP_CLEAR_COMMIT_RAW => state.keymap_commit_raw,
        ID_KEYMAP_CLEAR_CANCEL => state.keymap_cancel,
        ID_KEYMAP_CLEAR_CANCEL_ALL => state.keymap_cancel_all,
        ID_KEYMAP_CLEAR_MODE_HIRAGANA => state.keymap_mode_hiragana,
        ID_KEYMAP_CLEAR_MODE_KATAKANA => state.keymap_mode_katakana,
        ID_KEYMAP_CLEAR_MODE_ALPHANUMERIC => state.keymap_mode_alphanumeric,
        _ => return,
    };
    set_control_text(target, "");
}

fn show_error_message(hwnd: HWND, message: &str) {
    unsafe {
        let text = to_wide_z(message);
        let caption = to_wide_z(APP_TITLE);
        let _ = MessageBoxW(
            hwnd,
            PCWSTR(text.as_ptr()),
            PCWSTR(caption.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn show_info_message(hwnd: HWND, message: &str) {
    unsafe {
        let text = to_wide_z(message);
        let caption = to_wide_z(APP_TITLE);
        let _ = MessageBoxW(
            hwnd,
            PCWSTR(text.as_ptr()),
            PCWSTR(caption.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keymap_settings_use_preset_defaults() {
        let value: Value = r#"
preset = "ms-ime-jis"
inherit_preset = true
"#
        .parse()
        .expect("parse toml");
        let keymap = load_keymap_settings_from_value(&value);
        assert_eq!(keymap.ime_toggle, "Zenkaku");
        assert_eq!(keymap.mode_alphanumeric, "Eisuu");
    }

    #[test]
    fn saving_managed_bindings_preserves_unmanaged_entries() {
        let mut value: Value = r#"
preset = "ms-ime-jis"
inherit_preset = true

[[bindings]]
key = "Ctrl+Space"
action = "ime_toggle"

[[bindings]]
key = "Down"
action = "candidate_next"
"#
        .parse()
        .expect("parse toml");

        let settings = KeymapSettings {
            preset: "ms-ime-jis".to_string(),
            inherit_preset: true,
            ime_toggle: "Zenkaku".to_string(),
            convert: "Space".to_string(),
            commit_raw: "Enter".to_string(),
            cancel: "Escape".to_string(),
            cancel_all: "Ctrl+Backspace".to_string(),
            mode_hiragana: "Hiragana_key".to_string(),
            mode_katakana: "Katakana".to_string(),
            mode_alphanumeric: "Eisuu".to_string(),
        };

        update_managed_bindings(&mut value, &settings);
        let bindings = value
            .get("bindings")
            .and_then(Value::as_array)
            .expect("bindings array");
        let entries: Vec<_> = bindings.iter().filter_map(binding_entry).collect();

        assert!(
            entries
                .iter()
                .any(|(key, action)| key == "Down" && action == "candidate_next")
        );
        assert!(
            entries
                .iter()
                .any(|(key, action)| key == "Zenkaku" && action == "ime_toggle")
        );
        assert!(
            entries
                .iter()
                .any(|(key, action)| key == "Ctrl+Space" && action == "ime_toggle")
        );
    }

    #[test]
    fn key_binding_validator_matches_supported_names() {
        assert!(is_valid_key_binding("Ctrl+Space"));
        assert!(is_valid_key_binding("Henkan"));
        assert!(is_valid_key_binding("Alt+Caps"));
        assert!(!is_valid_key_binding("Ctrl+1"));
        assert!(!is_valid_key_binding("Ctrl++"));
    }

    // TODO: `normalize_key_capture` テストが削除されています。
    // 実装 (Ctrl+Shift+Right を Ctrl+Space に正規化するキー入力正規化処理) が
    // 0.6.0 時点でコミットされていなかったため、孤児テストとして 0.6.1 で
    // 撤去しました。キー正規化機能を追加する際に再実装 + テスト追加してください。
}
