//! 書き込み用のスレッド（ログの見直しの段 5、#76）。
//!
//! キー処理などの呼び出し元は、整形した行を上限つきのキューに入れるだけにする。サイズ確認・
//! 退避・書き込みは、このスレッドが行う。キューが満杯のときは待たずに捨て、捨てた行数を
//! 数えて、次に書けたときに 1 行で記録する。
//!
//! 制限: プロセスの終了時・落ちたときにキューに残った行は失われうる。終わる前に書き出せる
//! 場面（ホストの終了・panic）では [`Flusher::flush`] で待つ。

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::time::{Duration, Instant};

/// キューに積める行数。
pub const QUEUE_CAPACITY: usize = 8192;

#[derive(Default)]
struct Counters {
    /// キューに入れた行数（捨てた行を含まない）
    queued: AtomicU64,
    /// 書き込み用のスレッドが書いた行数
    written: AtomicU64,
    /// まだ記録していない、捨てた行数
    dropped: AtomicU64,
}

/// 書き込み用のスレッドへの口。`tracing_subscriber` の writer に渡せる。
#[derive(Clone)]
pub struct AsyncLog {
    tx: SyncSender<Vec<u8>>,
    counters: Arc<Counters>,
}

impl AsyncLog {
    /// `sink` に書くスレッドを起動する。`keep`（`.lock` など）はスレッドが終わるまで持つ。
    pub fn spawn<W, K>(name: &str, sink: W, keep: K) -> std::io::Result<Self>
    where
        W: Write + Send + 'static,
        K: Send + 'static,
    {
        Self::spawn_with_capacity(name, sink, keep, QUEUE_CAPACITY)
    }

    fn spawn_with_capacity<W, K>(
        name: &str,
        mut sink: W,
        keep: K,
        capacity: usize,
    ) -> std::io::Result<Self>
    where
        W: Write + Send + 'static,
        K: Send + 'static,
    {
        let (tx, rx) = sync_channel::<Vec<u8>>(capacity);
        let counters = Arc::new(Counters::default());
        let shared = counters.clone();
        std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                let _keep = keep;
                for line in rx {
                    // 書けなくても入力は止めない（RotatingLog は開けないときに捨てる）
                    let _ = sink.write_all(&line);
                    shared.written.fetch_add(1, Ordering::AcqRel);
                    let lost = shared.dropped.swap(0, Ordering::AcqRel);
                    if lost > 0 {
                        let _ = writeln!(sink, "rakukan-log: {lost} lines dropped (queue full)");
                    }
                }
            })?;
        Ok(Self { tx, counters })
    }

    /// 1 行（改行を含む）をキューに入れる。満杯なら待たずに捨てて数える。
    pub fn send(&self, line: Vec<u8>) {
        if line.is_empty() {
            return;
        }
        self.counters.queued.fetch_add(1, Ordering::AcqRel);
        if self.tx.try_send(line).is_err() {
            self.counters.queued.fetch_sub(1, Ordering::AcqRel);
            self.counters.dropped.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// キューを書き出すのを待つための口。
    pub fn flusher(&self) -> Flusher {
        Flusher(self.counters.clone())
    }
}

/// [`AsyncLog`] のキューが書き出されるのを待つ。
#[derive(Clone)]
pub struct Flusher(Arc<Counters>);

impl Flusher {
    /// 呼んだ時点でキューにある行が書かれるまで、`timeout` まで待つ。書き終えたら `true`。
    pub fn flush(&self, timeout: Duration) -> bool {
        let target = self.0.queued.load(Ordering::Acquire);
        let deadline = Instant::now() + timeout;
        loop {
            if self.0.written.load(Ordering::Acquire) >= target {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for AsyncLog {
    type Writer = Line<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        Line {
            log: self,
            buf: Vec::new(),
        }
    }
}

/// 1 件のイベントを整形する間だけ使うバッファ。drop でキューへ送る。
pub struct Line<'a> {
    log: &'a AsyncLog,
    buf: Vec<u8>,
}

impl Write for Line<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for Line<'_> {
    fn drop(&mut self) {
        self.log.send(std::mem::take(&mut self.buf));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::mpsc::Receiver;

    /// 書いた内容を共有のバッファへ積む sink。`gate` があれば 1 行ごとに許可を待つ。
    struct Sink {
        out: Arc<Mutex<Vec<u8>>>,
        gate: Option<Receiver<()>>,
    }

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if let Some(gate) = &self.gate {
                let _ = gate.recv();
            }
            self.out.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn text(out: &Arc<Mutex<Vec<u8>>>) -> String {
        String::from_utf8(out.lock().unwrap().clone()).unwrap()
    }

    #[test]
    fn lines_are_written_in_order_by_the_writer_thread() {
        let out = Arc::new(Mutex::new(Vec::new()));
        let sink = Sink {
            out: out.clone(),
            gate: None,
        };
        let log = AsyncLog::spawn("test-writer", sink, ()).unwrap();
        for i in 0..100 {
            log.send(format!("line {i}\n").into_bytes());
        }
        assert!(log.flusher().flush(Duration::from_secs(5)));
        let body = text(&out);
        let expected: String = (0..100).map(|i| format!("line {i}\n")).collect();
        assert_eq!(body, expected);
    }

    #[test]
    fn a_full_queue_drops_without_waiting_and_reports_the_count() {
        let out = Arc::new(Mutex::new(Vec::new()));
        let (open, gate) = std::sync::mpsc::channel();
        let sink = Sink {
            out: out.clone(),
            gate: Some(gate),
        };
        let log = AsyncLog::spawn_with_capacity("test-full", sink, (), 2).unwrap();
        // sink は止めてあるので、受け付けるのはキューの 2 行とスレッドが受け取った 1 行まで。
        // 残りは待たずに捨てる
        let sent = Instant::now();
        log.send(b"first\n".to_vec());
        for i in 0..11 {
            log.send(format!("more {i}\n").into_bytes());
        }
        assert!(sent.elapsed() < Duration::from_secs(1), "満杯で待った");
        // 数は、最初の行を書き終えるまで（sink を開くまで）は報告されない
        let dropped = log.counters.dropped.load(Ordering::Acquire);
        assert!(dropped >= 9, "捨てた行を数えていない: {dropped}");
        // 書かせる
        for _ in 0..32 {
            let _ = open.send(());
        }
        assert!(log.flusher().flush(Duration::from_secs(5)));
        let body = text(&out);
        assert!(body.starts_with("first\n"), "{body:?}");
        assert!(
            body.contains(&format!(
                "rakukan-log: {dropped} lines dropped (queue full)"
            )),
            "{body:?}"
        );
    }

    #[test]
    fn the_tracing_writer_sends_one_line_per_event() {
        let out = Arc::new(Mutex::new(Vec::new()));
        let sink = Sink {
            out: out.clone(),
            gate: None,
        };
        let log = AsyncLog::spawn("test-tracing", sink, ()).unwrap();
        let flusher = log.flusher();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(log)
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("hello");
            tracing::warn!("world");
        });
        assert!(flusher.flush(Duration::from_secs(5)));
        let body = text(&out);
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2, "{body:?}");
        assert!(lines[0].contains("INFO") && lines[0].ends_with("hello"));
        assert!(lines[1].contains("WARN") && lines[1].ends_with("world"));
    }
}
