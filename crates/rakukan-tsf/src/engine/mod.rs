pub(super) mod client_action;
pub(super) mod composition;
pub(crate) mod config;
pub(super) mod input_mode;
pub(super) mod keymap;
pub(super) mod state;
pub(super) mod text_util;
pub(super) mod user_action;

// conv_cache は rakukan-engine DLL 内部に移動（DLL 境界を越えないため）
