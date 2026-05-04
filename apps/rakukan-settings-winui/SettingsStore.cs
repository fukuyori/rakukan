using System.Diagnostics;
using System.Globalization;
using Tomlyn;
using Tomlyn.Model;

namespace Rakukan.Settings.WinUI;

internal sealed class SettingsBundle
{
    public required SettingsData Config { get; init; }
    public required KeymapSettings Keymap { get; init; }
    public required List<UserDictEntry> UserDict { get; init; }
}

internal sealed class UserDictEntry
{
    public string Reading { get; set; } = string.Empty;
    public List<string> Surfaces { get; set; } = new();

    public string SurfacesJoined => string.Join("、", Surfaces);
}

internal sealed class SettingsData
{
    public string LogLevel { get; set; } = "debug";
    public string? GpuBackend { get; set; }
    public uint? NGpuLayers { get; set; }
    public int MainGpu { get; set; }
    public string? ModelVariant { get; set; }
    public uint? NumCandidates { get; set; }
    public uint ConversionBeamSize { get; set; } = 6;
    public string KeyboardLayout { get; set; } = "jis";
    public bool ReloadOnModeSwitch { get; set; } = true;
    public string DefaultMode { get; set; } = "alphanumeric";
    public bool RememberLastKanaMode { get; set; } = true;
    public string DigitWidth { get; set; } = "halfwidth";
    public bool AutoLearn { get; set; } = true;
    public bool LiveEnabled { get; set; }
    public ulong DebounceMs { get; set; } = 80;
    public bool UseLlm { get; set; }
    public bool PreferDictionaryFirst { get; set; } = true;
    public uint BeamSize { get; set; } = 1;
}

internal enum ManagedKeyAction
{
    ImeToggle,
    Convert,
    CommitRaw,
    Cancel,
    CancelAll,
    ModeHiragana,
    ModeKatakana,
    ModeAlphanumeric,
}

internal sealed class KeymapSettings
{
    public string Preset { get; set; } = "ms-ime-jis";
    public bool InheritPreset { get; set; } = true;
    public Dictionary<ManagedKeyAction, string> Bindings { get; } = new();
    public Dictionary<string, List<string>> ManagedExtras { get; } = new();

    public string GetBinding(ManagedKeyAction action)
    {
        return Bindings.TryGetValue(action, out var value) ? value : string.Empty;
    }

    public void SetBinding(ManagedKeyAction action, string value)
    {
        if (string.IsNullOrWhiteSpace(value))
        {
            Bindings.Remove(action);
            return;
        }

        Bindings[action] = value.Trim();
    }

    public static KeymapSettings CreateDefault(string preset, bool inheritPreset)
    {
        var settings = new KeymapSettings
        {
            Preset = preset,
            InheritPreset = inheritPreset,
        };

        if (inheritPreset)
        {
            foreach (var action in ManagedKeyActions.All)
            {
                var key = ManagedKeyActions.DefaultKey(action, preset);
                if (!string.IsNullOrEmpty(key))
                {
                    settings.Bindings[action] = key;
                }
            }
        }

        return settings;
    }
}

internal static class ManagedKeyActions
{
    public static IReadOnlyList<ManagedKeyAction> All { get; } =
    [
        ManagedKeyAction.ImeToggle,
        ManagedKeyAction.Convert,
        ManagedKeyAction.CommitRaw,
        ManagedKeyAction.Cancel,
        ManagedKeyAction.CancelAll,
        ManagedKeyAction.ModeHiragana,
        ManagedKeyAction.ModeKatakana,
        ManagedKeyAction.ModeAlphanumeric,
    ];

    public static string ActionName(ManagedKeyAction action) => action switch
    {
        ManagedKeyAction.ImeToggle => "ime_toggle",
        ManagedKeyAction.Convert => "convert",
        ManagedKeyAction.CommitRaw => "commit_raw",
        ManagedKeyAction.Cancel => "cancel",
        ManagedKeyAction.CancelAll => "cancel_all",
        ManagedKeyAction.ModeHiragana => "mode_hiragana",
        ManagedKeyAction.ModeKatakana => "mode_katakana",
        ManagedKeyAction.ModeAlphanumeric => "mode_alphanumeric",
        _ => throw new ArgumentOutOfRangeException(nameof(action)),
    };

    public static ManagedKeyAction? FromActionName(string? actionName) => actionName switch
    {
        "ime_toggle" => ManagedKeyAction.ImeToggle,
        "convert" => ManagedKeyAction.Convert,
        "commit_raw" => ManagedKeyAction.CommitRaw,
        "cancel" => ManagedKeyAction.Cancel,
        "cancel_all" => ManagedKeyAction.CancelAll,
        "mode_hiragana" => ManagedKeyAction.ModeHiragana,
        "mode_katakana" => ManagedKeyAction.ModeKatakana,
        "mode_alphanumeric" => ManagedKeyAction.ModeAlphanumeric,
        _ => null,
    };

    public static string DefaultKey(ManagedKeyAction action, string preset) => (preset, action) switch
    {
        ("ms-ime-us", ManagedKeyAction.ImeToggle) => "Ctrl+Space",
        ("ms-ime-us", ManagedKeyAction.Convert) => "Space",
        ("ms-ime-us", ManagedKeyAction.CommitRaw) => "Enter",
        ("ms-ime-us", ManagedKeyAction.Cancel) => "Escape",
        ("ms-ime-us", ManagedKeyAction.CancelAll) => "Ctrl+Backspace",
        ("ms-ime-us", ManagedKeyAction.ModeHiragana) => "Ctrl+J",
        ("ms-ime-us", ManagedKeyAction.ModeKatakana) => "Ctrl+K",
        ("ms-ime-us", ManagedKeyAction.ModeAlphanumeric) => "Ctrl+L",
        ("ms-ime-jis", ManagedKeyAction.ImeToggle) => "Zenkaku",
        ("ms-ime-jis", ManagedKeyAction.Convert) => "Space",
        ("ms-ime-jis", ManagedKeyAction.CommitRaw) => "Enter",
        ("ms-ime-jis", ManagedKeyAction.Cancel) => "Escape",
        ("ms-ime-jis", ManagedKeyAction.CancelAll) => "Ctrl+Backspace",
        ("ms-ime-jis", ManagedKeyAction.ModeHiragana) => "Hiragana_key",
        ("ms-ime-jis", ManagedKeyAction.ModeKatakana) => "Katakana",
        ("ms-ime-jis", ManagedKeyAction.ModeAlphanumeric) => "Eisuu",
        _ => string.Empty,
    };
}

internal sealed class SettingsStore
{
    private const string DefaultConfigText = """
        # rakukan 設定ファイル
        # 入力モード変更時に再読込されます。

        [general]
        log_level = "debug"

        [keyboard]
        layout = "jis"
        reload_on_mode_switch = true

        [input]
        default_mode = "alphanumeric"
        remember_last_kana_mode = true
        digit_width = "halfwidth"
        digit_separator_auto = true
        digit_candidates_order = ["arabic", "fullwidth", "positional", "per_digit", "daiji"]
        auto_learn = true

        [live_conversion]
        enabled = false
        debounce_ms = 80
        use_llm = false
        prefer_dictionary_first = true
        beam_size = 1

        [conversion]
        beam_size = 6

        [diagnostics]
        dump_active_config = true
        warn_on_unknown_key = true
        """;

    private const string DefaultKeymapText = """
        # rakukan キーバインド設定
        # 入力モード変更時に再読込されます

        preset = "ms-ime-jis"
        inherit_preset = true
        """;

    public string ConfigPath => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
        "rakukan",
        "config.toml");

    public string KeymapPath => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
        "rakukan",
        "keymap.toml");

    public string UserDictPath => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
        "rakukan",
        "user_dict.toml");

    public SettingsBundle Load()
    {
        EnsureFile(ConfigPath, DefaultConfigText);
        EnsureFile(KeymapPath, DefaultKeymapText);

        var configTable = LoadToml(ConfigPath);
        var keymapTable = LoadToml(KeymapPath);
        var userDict = LoadUserDict(UserDictPath);

        return new SettingsBundle
        {
            Config = LoadConfig(configTable),
            Keymap = LoadKeymap(keymapTable),
            UserDict = userDict,
        };
    }

    /// <summary>
    /// 設定を保存する。戻り値 true = 少なくとも 1 ファイルが実際にディスク上で書き換わった。
    /// 戻り値 false = 全ファイルが既存の内容と完全一致で書き込みをスキップ（= エンジン reload 不要）。
    /// </summary>
    public bool Save(SettingsBundle bundle)
    {
        EnsureDirectory(ConfigPath);
        EnsureDirectory(KeymapPath);

        var configTable = LoadToml(ConfigPath);
        SaveConfig(configTable, bundle.Config);
        var configText = Toml.FromModel(configTable);
        var configChanged = WriteIfDifferent(ConfigPath, configText);

        var keymapTable = LoadToml(KeymapPath);
        SaveKeymap(keymapTable, bundle.Keymap);
        var keymapText = Toml.FromModel(keymapTable);
        var keymapChanged = WriteIfDifferent(KeymapPath, keymapText);

        var userDictChanged = SaveUserDict(UserDictPath, bundle.UserDict);

        return configChanged || keymapChanged || userDictChanged;
    }

    private static bool WriteIfDifferent(string path, string contents)
    {
        // Tomlyn の Toml.FromModel は LF 改行のみで出力するため、Windows 既定の
        // CRLF に揃える。既存ファイルが CRLF の状態で LF を書くと混在 (last
        // line だけ LF になる等) し、ユーザがエディタで開いたときに改行コード
        // 不統一の警告が出る。比較も正規化後の文字列で行うことで、CRLF→CRLF
        // の冪等書き込みを spurious change と誤判定しない。
        var normalized = NormalizeToCrlf(contents);
        if (File.Exists(path))
        {
            var existing = File.ReadAllText(path);
            if (string.Equals(existing, normalized, StringComparison.Ordinal))
            {
                return false;
            }
        }
        File.WriteAllText(path, normalized);
        return true;
    }

    private static string NormalizeToCrlf(string text)
    {
        // CRLF / LF / 単独 CR が混在しても最終的に CRLF へ統一する。
        // Replace の連鎖は: まず CRLF→LF で重複を潰し、CR→LF で残りの CR を吸収、
        // 最後に LF→CRLF。
        return text.Replace("\r\n", "\n").Replace("\r", "\n").Replace("\n", "\r\n");
    }

    public void OpenConfig() => OpenInNotepad(ConfigPath);

    public void OpenKeymap() => OpenInNotepad(KeymapPath);

    private static List<UserDictEntry> LoadUserDict(string path)
    {
        if (!File.Exists(path))
        {
            return new List<UserDictEntry>();
        }

        var root = LoadToml(path);
        var result = new List<UserDictEntry>();

        if (!root.TryGetValue("entries", out var entriesValue) || entriesValue is not TomlTableArray entries)
        {
            return result;
        }

        foreach (var item in entries.OfType<TomlTable>())
        {
            var reading = GetString(item, "reading");
            if (string.IsNullOrWhiteSpace(reading))
            {
                continue;
            }

            var surfaces = new List<string>();
            if (item.TryGetValue("surfaces", out var surfacesValue) && surfacesValue is TomlArray array)
            {
                foreach (var s in array.OfType<string>())
                {
                    if (!string.IsNullOrWhiteSpace(s))
                    {
                        surfaces.Add(s);
                    }
                }
            }

            if (surfaces.Count == 0)
            {
                continue;
            }

            result.Add(new UserDictEntry
            {
                Reading = reading!.Trim(),
                Surfaces = surfaces,
            });
        }

        return result;
    }

    private static bool SaveUserDict(string path, List<UserDictEntry> entries)
    {
        EnsureDirectory(path);

        var root = new TomlTable();
        var array = new TomlTableArray();

        foreach (var entry in entries)
        {
            if (string.IsNullOrWhiteSpace(entry.Reading) || entry.Surfaces.Count == 0)
            {
                continue;
            }

            var table = new TomlTable
            {
                ["reading"] = entry.Reading.Trim(),
            };
            var surfaces = new TomlArray();
            foreach (var s in entry.Surfaces)
            {
                if (!string.IsNullOrWhiteSpace(s))
                {
                    surfaces.Add(s);
                }
            }
            table["surfaces"] = surfaces;
            array.Add(table);
        }

        root["entries"] = array;
        return WriteIfDifferent(path, Toml.FromModel(root));
    }

    public void OpenUserDict()
    {
        EnsureFile(UserDictPath, "# rakukan ユーザー辞書\n# [[entries]] 形式で reading / surfaces を定義します。\n");
        Process.Start(new ProcessStartInfo("notepad.exe", $"\"{UserDictPath}\"")
        {
            UseShellExecute = true,
        });
    }

    private static void OpenInNotepad(string path)
    {
        EnsureFile(path, path.EndsWith("keymap.toml", StringComparison.OrdinalIgnoreCase) ? DefaultKeymapText : DefaultConfigText);
        Process.Start(new ProcessStartInfo("notepad.exe", $"\"{path}\"")
        {
            UseShellExecute = true,
        });
    }

    private static TomlTable LoadToml(string path)
    {
        var text = File.ReadAllText(path);
        return Toml.ToModel(text) as TomlTable ?? new TomlTable();
    }

    private static SettingsData LoadConfig(TomlTable root)
    {
        var general = GetOrCreateTable(root, "general");
        var keyboard = GetOrCreateTable(root, "keyboard");
        var input = GetOrCreateTable(root, "input");
        var live = GetOrCreateTable(root, "live_conversion");
        var conversion = GetOrCreateTable(root, "conversion");

        return new SettingsData
        {
            LogLevel = GetString(general, "log_level") ?? "info",
            GpuBackend = NormalizeOptional(GetString(general, "gpu_backend")),
            NGpuLayers = GetUInt(general, "n_gpu_layers"),
            MainGpu = GetInt(general, "main_gpu") ?? 0,
            ModelVariant = NormalizeOptional(GetString(general, "model_variant")),
            NumCandidates = GetUInt(conversion, "num_candidates") ?? GetUInt(root, "num_candidates"),
            ConversionBeamSize = GetUInt(conversion, "beam_size") ?? 6,
            KeyboardLayout = GetString(keyboard, "layout") ?? "jis",
            ReloadOnModeSwitch = GetBool(keyboard, "reload_on_mode_switch") ?? true,
            DefaultMode = GetString(input, "default_mode") ?? "alphanumeric",
            RememberLastKanaMode = GetBool(input, "remember_last_kana_mode") ?? true,
            DigitWidth = GetString(input, "digit_width") ?? "halfwidth",
            AutoLearn = GetBool(input, "auto_learn") ?? true,
            LiveEnabled = GetBool(live, "enabled") ?? false,
            DebounceMs = GetULong(live, "debounce_ms") ?? 80,
            UseLlm = GetBool(live, "use_llm") ?? false,
            PreferDictionaryFirst = GetBool(live, "prefer_dictionary_first") ?? true,
            BeamSize = GetUInt(live, "beam_size") ?? 1,
        };
    }

    private static void SaveConfig(TomlTable root, SettingsData data)
    {
        var general = GetOrCreateTable(root, "general");
        var keyboard = GetOrCreateTable(root, "keyboard");
        var input = GetOrCreateTable(root, "input");
        var live = GetOrCreateTable(root, "live_conversion");
        var conversion = GetOrCreateTable(root, "conversion");

        general["log_level"] = data.LogLevel;
        SetOptional(general, "gpu_backend", data.GpuBackend);
        SetOptional(general, "n_gpu_layers", data.NGpuLayers);
        general["main_gpu"] = data.MainGpu;
        SetOptional(general, "model_variant", data.ModelVariant);

        keyboard["layout"] = data.KeyboardLayout;
        keyboard["reload_on_mode_switch"] = data.ReloadOnModeSwitch;

        input["default_mode"] = data.DefaultMode;
        input["remember_last_kana_mode"] = data.RememberLastKanaMode;
        input["digit_width"] = data.DigitWidth;
        input["auto_learn"] = data.AutoLearn;

        live["enabled"] = data.LiveEnabled;
        live["debounce_ms"] = data.DebounceMs;
        live["use_llm"] = data.UseLlm;
        live["prefer_dictionary_first"] = data.PreferDictionaryFirst;
        live["beam_size"] = data.BeamSize;

        SetOptional(conversion, "num_candidates", data.NumCandidates);
        conversion["beam_size"] = data.ConversionBeamSize;
        root.Remove("num_candidates");
    }

    private static KeymapSettings LoadKeymap(TomlTable root)
    {
        var preset = GetString(root, "preset") ?? "ms-ime-jis";
        var inheritPreset = GetBool(root, "inherit_preset") ?? true;
        var settings = KeymapSettings.CreateDefault(preset, inheritPreset);
        var seenActions = new HashSet<ManagedKeyAction>();

        if (root.TryGetValue("bindings", out var bindingsValue) && bindingsValue is TomlTableArray bindings)
        {
            foreach (var item in bindings.OfType<TomlTable>())
            {
                var key = GetString(item, "key");
                var actionName = GetString(item, "action");
                var action = ManagedKeyActions.FromActionName(actionName);
                if (string.IsNullOrWhiteSpace(key) || action is null)
                {
                    continue;
                }

                if (seenActions.Add(action.Value))
                {
                    settings.Bindings[action.Value] = key!;
                }
                else
                {
                    AddManagedExtra(settings, action.Value, key!);
                }
            }
        }

        return settings;
    }

    private static void SaveKeymap(TomlTable root, KeymapSettings settings)
    {
        root["preset"] = settings.Preset;
        root["inherit_preset"] = settings.InheritPreset;

        var preserved = new List<TomlTable>();
        if (root.TryGetValue("bindings", out var existingValue) && existingValue is TomlTableArray existingBindings)
        {
            foreach (var item in existingBindings.OfType<TomlTable>())
            {
                var action = ManagedKeyActions.FromActionName(GetString(item, "action"));
                if (action is null)
                {
                    preserved.Add(item);
                }
            }
        }

        var newBindings = new TomlTableArray();
        foreach (var table in preserved)
        {
            newBindings.Add(table);
        }

        foreach (var action in ManagedKeyActions.All)
        {
            var primary = settings.GetBinding(action);
            if (!string.IsNullOrWhiteSpace(primary))
            {
                newBindings.Add(new TomlTable
                {
                    ["key"] = primary,
                    ["action"] = ManagedKeyActions.ActionName(action),
                });
            }

            if (settings.ManagedExtras.TryGetValue(ManagedKeyActions.ActionName(action), out var extras))
            {
                foreach (var extra in extras.Where(extra => !string.Equals(extra, primary, StringComparison.OrdinalIgnoreCase)))
                {
                    newBindings.Add(new TomlTable
                    {
                        ["key"] = extra,
                        ["action"] = ManagedKeyActions.ActionName(action),
                    });
                }
            }
        }

        root["bindings"] = newBindings;
    }

    private static string? GetString(TomlTable table, string key)
    {
        return table.TryGetValue(key, out var value) ? value as string : null;
    }

    private static bool? GetBool(TomlTable table, string key)
    {
        return table.TryGetValue(key, out var value) && value is bool flag ? flag : null;
    }

    private static uint? GetUInt(TomlTable table, string key)
    {
        if (!table.TryGetValue(key, out var value))
        {
            return null;
        }

        return value switch
        {
            long integer when integer >= 0 => (uint)integer,
            int integer when integer >= 0 => (uint)integer,
            _ => null,
        };
    }

    private static ulong? GetULong(TomlTable table, string key)
    {
        if (!table.TryGetValue(key, out var value))
        {
            return null;
        }

        return value switch
        {
            long integer when integer >= 0 => (ulong)integer,
            int integer when integer >= 0 => (ulong)integer,
            _ => null,
        };
    }

    private static int? GetInt(TomlTable table, string key)
    {
        if (!table.TryGetValue(key, out var value))
        {
            return null;
        }

        return value switch
        {
            long integer => (int)integer,
            int integer => integer,
            _ => null,
        };
    }

    private static TomlTable GetOrCreateTable(TomlTable root, string key)
    {
        if (root.TryGetValue(key, out var value) && value is TomlTable table)
        {
            return table;
        }

        var created = new TomlTable();
        root[key] = created;
        return created;
    }

    private static void SetOptional<T>(TomlTable table, string key, T? value)
    {
        if (value is null)
        {
            table.Remove(key);
        }
        else
        {
            table[key] = value;
        }
    }

    private static string? NormalizeOptional(string? value)
    {
        return string.IsNullOrWhiteSpace(value) ? null : value.Trim();
    }

    private static void AddManagedExtra(KeymapSettings settings, ManagedKeyAction action, string key)
    {
        var actionKey = ManagedKeyActions.ActionName(action);
        if (!settings.ManagedExtras.TryGetValue(actionKey, out var extras))
        {
            extras = new List<string>();
            settings.ManagedExtras[actionKey] = extras;
        }

        extras.Add(key);
    }

    private static void EnsureDirectory(string path)
    {
        var directory = Path.GetDirectoryName(path);
        if (!string.IsNullOrWhiteSpace(directory))
        {
            Directory.CreateDirectory(directory);
        }
    }

    private static void EnsureFile(string path, string defaultText)
    {
        EnsureDirectory(path);
        if (!File.Exists(path))
        {
            File.WriteAllText(path, NormalizeToCrlf(defaultText));
        }
    }
}
