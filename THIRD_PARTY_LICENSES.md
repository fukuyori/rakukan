# Third-Party Licenses

This document lists the licenses of third-party software and data used by rakukan.

---

## Conversion Engine

### karukan

- **Author:** Hitoshi Togasaki
- **Source:** https://github.com/togatoga/karukan
- **License:** MIT

```
MIT License

Copyright (c) Hitoshi Togasaki

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

---

## TSF Layer Reference

### azooKey-Windows

- **Author:** fkunn1326
- **Source:** https://github.com/fkunn1326/azooKey-Windows
- **License:** MIT

```
MIT License

Copyright (c) fkunn1326

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

---

## Dictionaries

> The following dictionary files are **downloaded at install time** from their
> respective repositories. They are **not** included in the rakukan source code
> or binary distribution.

### Mozc Open Source Dictionary

- **Author:** Google Inc.
- **Source:** https://github.com/google/mozc
- **Files:** `src/data/dictionary_oss/dictionary*.txt`, `reading_correction.tsv`
- **Usage:** Downloaded at install time, converted to `rakukan.dict` binary format
- **License:** Apache License 2.0

```
Copyright (c) 2010-2024, Google Inc.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
```

### SKK-JISYO.L

- **Author:** SKK Development Team
- **Source:** https://github.com/skk-dev/dict
- **Usage:** Downloaded at install time, used as fallback dictionary
- **License:** GNU General Public License v2

The full text of the GPL v2 is available at:
https://www.gnu.org/licenses/old-licenses/gpl-2.0.html

> **Note:** SKK-JISYO.L is used only as a runtime fallback dictionary and is
> downloaded directly from the upstream repository at install time. It is not
> redistributed as part of rakukan.

---

## Rust Dependencies

The following crates are used as dependencies. Each is distributed under its
stated license; full license texts are available via `cargo license` or at
https://crates.io.

| Crate | License |
|-------|---------|
| llama-cpp-2 | MIT |
| llama.cpp (via llama-cpp-2) | MIT |
| yada | MIT |
| memmap2 | MIT OR Apache-2.0 |
| windows (windows-rs) | MIT OR Apache-2.0 |
| tokenizers | Apache-2.0 |
| encoding_rs | MIT OR Apache-2.0 |
| anyhow | MIT OR Apache-2.0 |
| thiserror | MIT OR Apache-2.0 |
| tracing | MIT |
| tracing-subscriber | MIT |
| serde / serde_json | MIT OR Apache-2.0 |
| toml | MIT OR Apache-2.0 |
| clap | MIT OR Apache-2.0 |
| hf-hub | Apache-2.0 |
| unicode-normalization | MIT OR Apache-2.0 |

To generate a full dependency license report:

```powershell
cargo install cargo-license
cargo license
```
