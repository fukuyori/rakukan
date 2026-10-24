//! 調査用の ETW provider（ログの見直しの段 5、#76）。
//!
//! ファイルのログを主にし、ETW は調査のときだけ使う。セッションは利用者が管理者の権限で
//! 手で開始・停止する（`scripts/etw-trace.ps1`）。セッションが無い間は `enabled()` が偽になり、
//! イベントを組み立てない。
//!
//! - provider は TSF（[`TSF_PROVIDER`]）とホスト（[`HOST_PROVIDER`]）。engine DLL のイベントは
//!   ホストへの callback を通り、`Rakukan.Host` に target `rakukan_engine_dll` で出る。
//! - 出す範囲はファイルのログと同じ（同じフィルターを通ったイベント）。
//! - 登録した provider は解除しない。TSF DLL は Activate の後に PIN されて外れず、ホストは
//!   EXE なので、プロセスの終了まで使う（登録したまま DLL が外れると ETW の callback で落ちる）。
//! - 制限: RPC をまたぐ activity の ID は付けない。要求の番号（`seq`）で追う。

/// TSF の provider 名。GUID は名前から決まる（[`TSF_PROVIDER_GUID`]）。
pub const TSF_PROVIDER: &str = "Rakukan.Tsf";
/// ホストの provider 名（engine DLL のイベントを含む）。
pub const HOST_PROVIDER: &str = "Rakukan.Host";

/// `Guid::from_name(TSF_PROVIDER)`。`scripts/etw-trace.ps1` と同じ値にする（下の試験で名前から計算した値と比べる）。
pub const TSF_PROVIDER_GUID: &str = "d28ecef7-3b1a-5580-2ec2-9e71c758c5ef";
/// `Guid::from_name(HOST_PROVIDER)`。
pub const HOST_PROVIDER_GUID: &str = "f3736713-19a4-5666-aab9-bd2a1ec6513c";

/// tracing のイベントを ETW へ出す Layer。
pub struct EtwLayer {
    #[cfg(windows)]
    provider: &'static std::pin::Pin<Box<tracelogging_dynamic::Provider>>,
}

/// provider を使えない理由。呼び出し側がファイルのログに残す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EtwError {
    /// `EventRegister` が失敗した（Win32 のエラーコード）
    RegisterFailed(u32),
    /// このプロセスでは既に別の名前の provider を登録している
    OtherProviderRegistered { registered: String },
    /// Windows 以外
    Unsupported,
}

impl std::fmt::Display for EtwError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RegisterFailed(code) => write!(f, "EventRegister failed (error {code})"),
            Self::OtherProviderRegistered { registered } => {
                write!(f, "another provider is already registered: {registered}")
            }
            Self::Unsupported => f.write_str("ETW is not available on this platform"),
        }
    }
}

/// 登録した provider（プロセスにつき 1 つ）。名前と登録の結果も持つ。
#[cfg(windows)]
struct Registered {
    name: String,
    provider: std::pin::Pin<Box<tracelogging_dynamic::Provider>>,
    result: u32,
}

#[cfg(windows)]
static PROVIDER: std::sync::OnceLock<Registered> = std::sync::OnceLock::new();

impl EtwLayer {
    /// provider を登録して Layer を返す。プロセスにつき 1 つ。登録に失敗したとき、または既に
    /// 別の名前で登録しているときは理由を返す（呼び出し側はファイルのログだけで続ける）。
    #[cfg(windows)]
    pub fn register(name: &str) -> Result<Self, EtwError> {
        use tracelogging_dynamic::Provider;
        let registered = PROVIDER.get_or_init(|| {
            let provider = Box::pin(Provider::new(name, &Provider::options()));
            // SAFETY: static に置き、動かさず、解除もしない（このモジュールの説明を参照）
            let result = unsafe { provider.as_ref().register() };
            Registered {
                name: name.to_string(),
                provider,
                result,
            }
        });
        if registered.name != name {
            return Err(EtwError::OtherProviderRegistered {
                registered: registered.name.clone(),
            });
        }
        if registered.result != 0 {
            return Err(EtwError::RegisterFailed(registered.result));
        }
        Ok(Self {
            provider: &registered.provider,
        })
    }

    #[cfg(not(windows))]
    pub fn register(_name: &str) -> Result<Self, EtwError> {
        Err(EtwError::Unsupported)
    }
}

#[cfg(windows)]
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for EtwLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        use tracelogging_dynamic::{EventBuilder, Level, OutType};
        let meta = event.metadata();
        let level = match *meta.level() {
            tracing::Level::ERROR => Level::Error,
            tracing::Level::WARN => Level::Warning,
            tracing::Level::INFO => Level::Informational,
            tracing::Level::DEBUG | tracing::Level::TRACE => Level::Verbose,
        };
        // セッションが無ければ組み立てない
        if !self.provider.enabled(level, 0) {
            return;
        }
        let mut text = MessageText::default();
        event.record(&mut text);
        // PID・TID・時刻は ETW が付ける
        EventBuilder::new()
            .reset(meta.name(), level, 0, 0)
            .add_str8("level", meta.level().as_str(), OutType::Utf8, 0)
            .add_str8("target", meta.target(), OutType::Utf8, 0)
            .add_str8("message", &text.0, OutType::Utf8, 0)
            .write(self.provider, None, None);
    }
}

#[cfg(not(windows))]
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for EtwLayer {}

/// イベントの本文（`message`）と、ほかのフィールド（` name=value`）を 1 行にする。
#[derive(Default)]
struct MessageText(String);

impl tracing::field::Visit for MessageText {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0.insert_str(0, value);
        } else {
            use std::fmt::Write;
            let _ = write!(self.0, " {}={value}", field.name());
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        if field.name() == "message" {
            self.0.insert_str(0, &format!("{value:?}"));
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn provider_guids_are_derived_from_the_names() {
        let guid = |name| {
            format!("{:?}", tracelogging_dynamic::Provider::guid_from_name(name)).to_lowercase()
        };
        assert_eq!(guid(TSF_PROVIDER), TSF_PROVIDER_GUID);
        assert_eq!(guid(HOST_PROVIDER), HOST_PROVIDER_GUID);
    }

    #[test]
    fn events_without_a_session_are_skipped() {
        use tracing_subscriber::layer::SubscriberExt;
        let layer = EtwLayer::register("Rakukan.Test").expect("layer");
        let subscriber = tracing_subscriber::registry().with(layer);
        // セッションが無いので何も出ないが、落ちない
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(reading = "かな", "converted");
        });
        // 同じプロセスで別の名前を渡すと、登録済みの provider を黙って使わずに理由を返す
        assert_eq!(
            EtwLayer::register("Rakukan.Other").err(),
            Some(EtwError::OtherProviderRegistered {
                registered: "Rakukan.Test".to_string()
            })
        );
        assert!(EtwLayer::register("Rakukan.Test").is_ok());
    }
}
