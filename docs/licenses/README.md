# 上流の LICENSE（crate に入っていないもの）

crate にライセンスのファイルが入っていないと、cargo-about は著作権者の名前の無い標準の本文で埋める。著作権の表示が条件のライセンス（MIT・BSD・ISC）でそうなる crate は、上流の LICENSE をここに `<名前>-<版>.txt` で置く。`scripts/gen-third-party-licenses.ps1` がこの本文を `THIRD_PARTY_LICENSES.md` に使い、置いていない crate があれば `-Check` が失敗する。

取ったところは、crate の `.cargo_vcs_info.json` が示す、crate を作ったコミット。

| ファイル | 取ったところ |
|---|---|
| `tracelogging-1.2.4.txt` | https://github.com/microsoft/tracelogging の `bdd914bd827ff6b9d6d65972d622834fdf04518a` の `LICENSE` |
| `tracelogging_dynamic-1.2.4.txt` | 同じリポジトリ・同じコミットの `LICENSE`（crate は `etw/rust/tracelogging_dynamic`） |
| `number_prefix-0.4.0.txt` | https://github.com/ogham/rust-number-prefix の `eb6ebd215d50df1b199737f0356b988bebaedc84` の `LICENCE` |
