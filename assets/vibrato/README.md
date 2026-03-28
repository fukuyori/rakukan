Bundled Vibrato dictionary files live here.

- `assets/vibrato/system.dic`

Phase 1 expects the installer or local install script to copy that file to:

- `%LOCALAPPDATA%\\rakukan\\dict\\vibrato\\system.dic`

If the file is missing, rakukan falls back to the existing heuristic split logic.

To rebuild `system.dic` from the bundled MeCab IPADIC source:

- `powershell -ExecutionPolicy Bypass -File scripts/build-vibrato-dict.ps1`
