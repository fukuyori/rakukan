using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using System.Threading;
using Windows.Graphics;

namespace Rakukan.Settings.WinUI;

public sealed partial class MainWindow : Window
{
    private const string ReloadEventName = @"Local\rakukan.engine.reload";

    private readonly SettingsStore _store = new();
    private readonly Dictionary<ManagedKeyAction, TextBox> _keyFields;
    private readonly SettingsBundle _settings;
    private bool _isClosingAfterApply;

    public MainWindow()
    {
        InitializeComponent();
        AppWindow.Resize(new SizeInt32(1080, 760));
        AppWindow.Closing += AppWindow_Closing;

        _keyFields = new Dictionary<ManagedKeyAction, TextBox>
        {
            [ManagedKeyAction.ImeToggle] = ImeToggleBox,
            [ManagedKeyAction.Convert] = ConvertBox,
            [ManagedKeyAction.CommitRaw] = CommitRawBox,
            [ManagedKeyAction.Cancel] = CancelBox,
            [ManagedKeyAction.CancelAll] = CancelAllBox,
            [ManagedKeyAction.ModeHiragana] = ModeHiraganaBox,
            [ManagedKeyAction.ModeKatakana] = ModeKatakanaBox,
            [ManagedKeyAction.ModeAlphanumeric] = ModeAlphanumericBox,
        };

        _settings = _store.Load();
        ApplySettingsToUi(_settings);

        if (RootNavigation.MenuItems.OfType<NavigationViewItem>().FirstOrDefault() is { } first)
        {
            RootNavigation.SelectedItem = first;
            ShowPage(first.Tag?.ToString() ?? "General");
        }
    }

    private void ApplySettingsToUi(SettingsBundle bundle)
    {
        SelectComboValue(LogLevelCombo, bundle.Config.LogLevel);
        SelectComboValue(GpuBackendCombo, bundle.Config.GpuBackend ?? "auto");
        NGpuLayersBox.Value = bundle.Config.NGpuLayers ?? double.NaN;
        MainGpuBox.Value = bundle.Config.MainGpu;
        ModelVariantBox.Text = bundle.Config.ModelVariant ?? string.Empty;
        NumCandidatesBox.Value = bundle.Config.NumCandidates ?? double.NaN;

        SelectComboValue(KeyboardLayoutCombo, bundle.Config.KeyboardLayout);
        ReloadOnModeSwitchToggle.IsOn = bundle.Config.ReloadOnModeSwitch;
        SelectComboValue(DefaultModeCombo, bundle.Config.DefaultMode);
        RememberKanaModeToggle.IsOn = bundle.Config.RememberLastKanaMode;
        SelectComboValue(DigitWidthCombo, bundle.Config.DigitWidth);

        SelectComboValue(KeymapPresetCombo, bundle.Keymap.Preset);
        KeymapInheritToggle.IsOn = bundle.Keymap.InheritPreset;
        foreach (var action in ManagedKeyActions.All)
        {
            _keyFields[action].Text = bundle.Keymap.GetBinding(action);
        }

        LiveEnabledToggle.IsOn = bundle.Config.LiveEnabled;
        DebounceMsBox.Value = bundle.Config.DebounceMs;
        BeamSizeBox.Value = bundle.Config.BeamSize;
        UseLlmToggle.IsOn = bundle.Config.UseLlm;
        PreferDictionaryFirstToggle.IsOn = bundle.Config.PreferDictionaryFirst;
    }

    private SettingsBundle CaptureSettingsFromUi()
    {
        var numCandidates = ParseOptionalUInt(NumCandidatesBox.Value, "候補数", 1, 9);
        var beamSize = ParseUInt(BeamSizeBox.Value, "beam_size", 1, 9);

        var config = new SettingsData
        {
            LogLevel = SelectedComboValue(LogLevelCombo),
            GpuBackend = NormalizeOptional(SelectedComboValue(GpuBackendCombo), "auto"),
            NGpuLayers = ParseOptionalUInt(NGpuLayersBox.Value, "GPU レイヤー数"),
            MainGpu = ParseInt(MainGpuBox.Value, "使用 GPU インデックス"),
            ModelVariant = NormalizeOptional(ModelVariantBox.Text, string.Empty),
            NumCandidates = numCandidates,
            KeyboardLayout = SelectedComboValue(KeyboardLayoutCombo),
            ReloadOnModeSwitch = ReloadOnModeSwitchToggle.IsOn,
            DefaultMode = SelectedComboValue(DefaultModeCombo),
            RememberLastKanaMode = RememberKanaModeToggle.IsOn,
            DigitWidth = SelectedComboValue(DigitWidthCombo),
            LiveEnabled = LiveEnabledToggle.IsOn,
            DebounceMs = ParseULong(DebounceMsBox.Value, "デバウンス"),
            UseLlm = UseLlmToggle.IsOn,
            PreferDictionaryFirst = PreferDictionaryFirstToggle.IsOn,
            BeamSize = beamSize,
        };

        var keymap = new KeymapSettings
        {
            Preset = SelectedComboValue(KeymapPresetCombo),
            InheritPreset = KeymapInheritToggle.IsOn,
        };

        foreach (var action in ManagedKeyActions.All)
        {
            keymap.SetBinding(
                action,
                ValidateKeyBinding(ActionLabel(action), _keyFields[action].Text));
        }

        foreach (var pair in _settings.Keymap.ManagedExtras)
        {
            keymap.ManagedExtras[pair.Key] = [.. pair.Value];
        }

        return new SettingsBundle
        {
            Config = config,
            Keymap = keymap,
        };
    }

    private void OnNavigationSelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        ShowPage(args.SelectedItemContainer?.Tag?.ToString() ?? "General");
    }

    private void ShowPage(string tag)
    {
        GeneralPage.Visibility = tag == "General" ? Visibility.Visible : Visibility.Collapsed;
        InputPage.Visibility = tag == "Input" ? Visibility.Visible : Visibility.Collapsed;
        KeysPage.Visibility = tag == "Keys" ? Visibility.Visible : Visibility.Collapsed;
        LivePage.Visibility = tag == "Live" ? Visibility.Visible : Visibility.Collapsed;
        AdvancedPage.Visibility = tag == "Advanced" ? Visibility.Visible : Visibility.Collapsed;
    }

    private void KeymapPresetCombo_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        ApplyKeymapPresetDefaults();
    }

    private void KeymapInheritToggle_Toggled(object sender, RoutedEventArgs e)
    {
        ApplyKeymapPresetDefaults();
    }

    private void ApplyKeymapPresetDefaults()
    {
        if (!KeymapInheritToggle.IsOn)
        {
            return;
        }

        var defaults = KeymapSettings.CreateDefault(SelectedComboValue(KeymapPresetCombo), true);
        foreach (var action in ManagedKeyActions.All)
        {
            _keyFields[action].Text = defaults.GetBinding(action);
        }
    }

    private void ClearKeyButton_Click(object sender, RoutedEventArgs e)
    {
        if (sender is not Button button || button.Tag is not string tag)
        {
            return;
        }

        if (Enum.TryParse<ManagedKeyAction>(tag, out var action))
        {
            _keyFields[action].Text = string.Empty;
        }
    }

    private async void SaveButton_Click(object sender, RoutedEventArgs e)
    {
        if (!TrySaveAndApply(out var error))
        {
            await ShowDialogAsync("設定を保存できませんでした", error);
        }
    }

    private async void CloseButton_Click(object sender, RoutedEventArgs e)
    {
        if (!TrySaveAndApply(out var error))
        {
            await ShowDialogAsync("設定を保存できませんでした", error);
            return;
        }

        _isClosingAfterApply = true;
        Close();
    }

    private async void OpenConfigButton_Click(object sender, RoutedEventArgs e)
    {
        try
        {
            _store.OpenConfig();
        }
        catch (Exception ex)
        {
            await ShowDialogAsync("config.toml を開けませんでした", ex.Message);
        }
    }

    private async void OpenKeymapButton_Click(object sender, RoutedEventArgs e)
    {
        try
        {
            _store.OpenKeymap();
        }
        catch (Exception ex)
        {
            await ShowDialogAsync("keymap.toml を開けませんでした", ex.Message);
        }
    }

    private async Task ShowDialogAsync(string title, string message)
    {
        var dialog = new ContentDialog
        {
            Title = title,
            Content = message,
            CloseButtonText = "閉じる",
            XamlRoot = Content.XamlRoot,
        };
        await dialog.ShowAsync();
    }

    private static void SelectComboValue(ComboBox comboBox, string value)
    {
        comboBox.SelectedItem = comboBox.Items.FirstOrDefault(item => string.Equals(item?.ToString(), value, StringComparison.OrdinalIgnoreCase));
    }

    private static string SelectedComboValue(ComboBox comboBox)
    {
        return comboBox.SelectedItem?.ToString() ?? string.Empty;
    }

    private static string? NormalizeOptional(string? value, string emptyAsNull)
    {
        if (string.IsNullOrWhiteSpace(value))
        {
            return null;
        }

        return string.Equals(value, emptyAsNull, StringComparison.OrdinalIgnoreCase) ? null : value.Trim();
    }

    private static string ActionLabel(ManagedKeyAction action) => action switch
    {
        ManagedKeyAction.ImeToggle => "IME 切替",
        ManagedKeyAction.Convert => "変換開始",
        ManagedKeyAction.CommitRaw => "ひらがな確定",
        ManagedKeyAction.Cancel => "取消",
        ManagedKeyAction.CancelAll => "全取消",
        ManagedKeyAction.ModeHiragana => "ひらがなモード",
        ManagedKeyAction.ModeKatakana => "カタカナモード",
        ManagedKeyAction.ModeAlphanumeric => "英数モード",
        _ => action.ToString(),
    };

    private static string ValidateKeyBinding(string label, string value)
    {
        var trimmed = value.Trim();
        if (string.IsNullOrEmpty(trimmed))
        {
            return string.Empty;
        }

        if (!IsValidKeyBinding(trimmed))
        {
            throw new InvalidOperationException(
                $"{label} は対応しているキー名で入力してください。例: Ctrl+Space, Henkan, Zenkaku, F6");
        }

        return trimmed;
    }

    private static bool IsValidKeyBinding(string value)
    {
        var sawKey = false;
        foreach (var part in value.Split('+', StringSplitOptions.None))
        {
            var token = part.Trim().ToLowerInvariant();
            if (string.IsNullOrEmpty(token))
            {
                return false;
            }

            if (token is "ctrl" or "control" or "shift" or "alt")
            {
                continue;
            }

            if (!sawKey && IsSupportedKeyName(token))
            {
                sawKey = true;
                continue;
            }

            return false;
        }

        return sawKey;
    }

    private static bool IsSupportedKeyName(string name)
    {
        return name switch
        {
            "backspace" or "bs" or "tab" or "enter" or "return" or "escape" or "esc"
                or "space" or "backquote" or "grave" or "semicolon" or "equal"
                or "comma" or "minus" or "period" or "slash" or "leftbracket"
                or "backslash" or "rightbracket" or "quote" or "pageup" or "pgup"
                or "pagedown" or "pgdn" or "end" or "home" or "left" or "up"
                or "right" or "down" or "delete" or "del" or "f1" or "f2" or "f3"
                or "f4" or "f5" or "f6" or "f7" or "f8" or "f9" or "f10" or "f11"
                or "f12" or "zenkaku" or "hankaku" or "kanji" or "henkan"
                or "muhenkan" or "eisuu" or "alphanumeric" or "katakana"
                or "hiragana_key" or "caps" => true,
            _ => name.Length == 1 && char.IsAsciiLetter(name[0]),
        };
    }

    private static uint? ParseOptionalUInt(double value, string label, uint? min = null, uint? max = null)
    {
        if (double.IsNaN(value))
        {
            return null;
        }

        var parsed = checked((uint)value);
        ValidateRange(parsed, label, min, max);
        return parsed;
    }

    private static uint ParseUInt(double value, string label, uint? min = null, uint? max = null)
    {
        if (double.IsNaN(value))
        {
            throw new InvalidOperationException($"{label} を入力してください。");
        }

        var parsed = checked((uint)value);
        ValidateRange(parsed, label, min, max);
        return parsed;
    }

    private static ulong ParseULong(double value, string label)
    {
        if (double.IsNaN(value))
        {
            throw new InvalidOperationException($"{label} を入力してください。");
        }

        return checked((ulong)value);
    }

    private static int ParseInt(double value, string label)
    {
        if (double.IsNaN(value))
        {
            throw new InvalidOperationException($"{label} を入力してください。");
        }

        return checked((int)value);
    }

    private static void ValidateRange(uint value, string label, uint? min, uint? max)
    {
        if (min.HasValue && value < min.Value || max.HasValue && value > max.Value)
        {
            throw new InvalidOperationException($"{label} は {min} から {max} の範囲で入力してください。");
        }
    }

    private async void AppWindow_Closing(AppWindow sender, AppWindowClosingEventArgs args)
    {
        if (_isClosingAfterApply)
        {
            return;
        }

        if (!TrySaveAndApply(out var error))
        {
            args.Cancel = true;
            await ShowDialogAsync("設定を保存できませんでした", error);
        }
    }

    private bool TrySaveAndApply(out string error)
    {
        try
        {
            var captured = CaptureSettingsFromUi();
            _store.Save(captured);
            SignalReload();

            StatusBar.Severity = InfoBarSeverity.Success;
            StatusBar.Title = "反映しました";
            StatusBar.Message = "設定を保存し、現在の IME に反映しました。";
            StatusBar.IsOpen = true;

            error = string.Empty;
            return true;
        }
        catch (Exception ex)
        {
            error = ex.Message;
            return false;
        }
    }

    private static void SignalReload()
    {
        try
        {
            using var reloadEvent = EventWaitHandle.OpenExisting(ReloadEventName);
            reloadEvent.Set();
        }
        catch (WaitHandleCannotBeOpenedException)
        {
            // IME 側の監視イベントがまだ作られていない場合は何もしない。
        }
    }

}
