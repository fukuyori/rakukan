using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using System.Collections.ObjectModel;
using System.IO;
using System.Threading;
using Windows.Graphics;

namespace Rakukan.Settings.WinUI;

public sealed partial class MainWindow : Window
{
    private const string ReloadEventName = @"Local\rakukan.engine.reload";
    private static readonly string AppIconPath = Path.Combine(AppContext.BaseDirectory, "rakukan.ico");

    private readonly SettingsStore _store = new();
    private readonly Dictionary<ManagedKeyAction, TextBox> _keyFields;
    private readonly SettingsBundle _settings;
    private readonly ObservableCollection<UserDictEntry> _userDictEntries = new();
    private bool _isClosingAfterApply;

    public MainWindow()
    {
        InitializeComponent();
        SetWindowIcon();
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
        UserDictList.ItemsSource = _userDictEntries;
        ApplySettingsToUi(_settings);

        if (RootNavigation.MenuItems.OfType<NavigationViewItem>().FirstOrDefault() is { } first)
        {
            RootNavigation.SelectedItem = first;
            ShowPage(first.Tag?.ToString() ?? "General");
        }
    }

    private void SetWindowIcon()
    {
        if (File.Exists(AppIconPath))
        {
            AppWindow.SetIcon(AppIconPath);
        }
    }

    private void ApplySettingsToUi(SettingsBundle bundle)
    {
        SelectComboValue(LogLevelCombo, bundle.Config.LogLevel);
        SelectComboValue(GpuBackendCombo, bundle.Config.GpuBackend ?? "auto");
        NGpuLayersBox.Value = bundle.Config.NGpuLayers ?? double.NaN;
        MainGpuBox.Value = bundle.Config.MainGpu;
        ModelVariantCombo.Text = bundle.Config.ModelVariant ?? string.Empty;
        // null の場合はデフォルト (9) を表示する。NaN だと WinUI NumberBox の
        // スピンボタンが動作せず、値が空で表示される問題があるため常に数値を入れる。
        NumCandidatesBox.Value = bundle.Config.NumCandidates ?? 9;

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

        _userDictEntries.Clear();
        foreach (var entry in bundle.UserDict)
        {
            _userDictEntries.Add(new UserDictEntry
            {
                Reading = entry.Reading,
                Surfaces = new List<string>(entry.Surfaces),
            });
        }
    }

    private SettingsBundle CaptureSettingsFromUi()
    {
        var numCandidates = ParseOptionalUInt(NumCandidatesBox.Value, "候補数", 1, 30);
        // デフォルト値 (9) のまま保存した場合は config.toml に書き込まない
        // （コメントアウト状態を維持して将来のデフォルト変更に追従しやすくする）
        if (numCandidates == 9)
        {
            numCandidates = null;
        }
        var beamSize = ParseUInt(BeamSizeBox.Value, "beam_size", 1, 9);

        var config = new SettingsData
        {
            LogLevel = SelectedComboValue(LogLevelCombo),
            GpuBackend = NormalizeOptional(SelectedComboValue(GpuBackendCombo), string.Empty),
            NGpuLayers = ParseOptionalUInt(NGpuLayersBox.Value, "GPU レイヤー数"),
            MainGpu = ParseInt(MainGpuBox.Value, "使用 GPU インデックス"),
            ModelVariant = NormalizeOptional(ReadModelVariantFromCombo(), string.Empty),
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

        var userDict = _userDictEntries
            .Select(e => new UserDictEntry
            {
                Reading = e.Reading,
                Surfaces = new List<string>(e.Surfaces),
            })
            .ToList();

        return new SettingsBundle
        {
            Config = config,
            Keymap = keymap,
            UserDict = userDict,
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
        UserDictPage.Visibility = tag == "UserDict" ? Visibility.Visible : Visibility.Collapsed;
        AdvancedPage.Visibility = tag == "Advanced" ? Visibility.Visible : Visibility.Collapsed;
    }

    private void KeymapPresetCombo_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        ApplyKeymapPresetDefaults();
    }

    // DispatcherQueue での遅延代入が万が一失敗しても、Text に混入した表示用
    // サフィックス ("xxx (約 NN MB)") を config.toml に書き出さないよう、
    // 保存時は必ず SelectedItem.Tag を優先する。Text が Tag のどれとも
    // マッチしない場合のみ Text を使う (将来の未知 variant を手入力する用途)。
    private string ReadModelVariantFromCombo()
    {
        if (ModelVariantCombo.SelectedItem is ComboBoxItem item && item.Tag is string variantId)
        {
            return variantId;
        }

        var text = ModelVariantCombo.Text ?? string.Empty;
        foreach (var obj in ModelVariantCombo.Items)
        {
            if (obj is ComboBoxItem candidate
                && candidate.Tag is string tag
                && string.Equals(text.Trim(), tag, StringComparison.Ordinal))
            {
                return tag;
            }
        }
        return text;
    }

    private void ModelVariantCombo_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        // ComboBoxItem.Content は "xxx (約 NN MB)" の表示用文字列。
        // IsEditable=True の ComboBox は SelectionChanged の **後** に
        // Text を Content で上書きするため、ここで即代入すると無効化される。
        // DispatcherQueue で遅延させ、WinUI の Text 更新後に Tag (variant ID) に置き換える。
        if (ModelVariantCombo.SelectedItem is ComboBoxItem item && item.Tag is string variantId)
        {
            DispatcherQueue.TryEnqueue(() => ModelVariantCombo.Text = variantId);
        }
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

    private async void OpenUserDictButton_Click(object sender, RoutedEventArgs e)
    {
        try
        {
            _store.OpenUserDict();
        }
        catch (Exception ex)
        {
            await ShowDialogAsync("user_dict.toml を開けませんでした", ex.Message);
        }
    }

    private async void UserDictAddButton_Click(object sender, RoutedEventArgs e)
    {
        var entry = await ShowUserDictEditorAsync(null);
        if (entry is not null)
        {
            _userDictEntries.Add(entry);
            UserDictList.SelectedItem = entry;
        }
    }

    private async void UserDictEditButton_Click(object sender, RoutedEventArgs e)
    {
        if (UserDictList.SelectedItem is not UserDictEntry current)
        {
            await ShowDialogAsync("編集する項目を選択してください", "リストから編集したい行を選んでから「編集」を押してください。");
            return;
        }
        await EditEntryAsync(current);
    }

    private void UserDictDeleteButton_Click(object sender, RoutedEventArgs e)
    {
        if (UserDictList.SelectedItem is UserDictEntry current)
        {
            _userDictEntries.Remove(current);
        }
    }

    private async void UserDictList_DoubleTapped(object sender, Microsoft.UI.Xaml.Input.DoubleTappedRoutedEventArgs e)
    {
        if (UserDictList.SelectedItem is UserDictEntry current)
        {
            await EditEntryAsync(current);
        }
    }

    private async Task EditEntryAsync(UserDictEntry current)
    {
        var edited = await ShowUserDictEditorAsync(current);
        if (edited is null)
        {
            return;
        }
        current.Reading = edited.Reading;
        current.Surfaces = edited.Surfaces;
        var index = _userDictEntries.IndexOf(current);
        if (index >= 0)
        {
            _userDictEntries.RemoveAt(index);
            _userDictEntries.Insert(index, current);
            UserDictList.SelectedItem = current;
        }
    }

    private async Task<UserDictEntry?> ShowUserDictEditorAsync(UserDictEntry? existing)
    {
        var readingBox = new TextBox
        {
            Header = "読み (ひらがな)",
            PlaceholderText = "例: きむら",
            Text = existing?.Reading ?? string.Empty,
        };
        var surfacesBox = new TextBox
        {
            Header = "変換候補 (1 行に 1 つ、先頭行が最優先)",
            PlaceholderText = "例:\n木村\n金村",
            Text = existing is null ? string.Empty : string.Join(Environment.NewLine, existing.Surfaces),
            AcceptsReturn = true,
            TextWrapping = Microsoft.UI.Xaml.TextWrapping.Wrap,
            MinHeight = 120,
        };

        var panel = new StackPanel { Spacing = 12 };
        panel.Children.Add(readingBox);
        panel.Children.Add(surfacesBox);

        var dialog = new ContentDialog
        {
            Title = existing is null ? "ユーザー辞書に追加" : "ユーザー辞書を編集",
            Content = panel,
            PrimaryButtonText = "OK",
            CloseButtonText = "キャンセル",
            DefaultButton = ContentDialogButton.Primary,
            XamlRoot = Content.XamlRoot,
        };

        var result = await dialog.ShowAsync();
        if (result != ContentDialogResult.Primary)
        {
            return null;
        }

        var reading = (readingBox.Text ?? string.Empty).Trim();
        var surfaces = (surfacesBox.Text ?? string.Empty)
            .Split(new[] { '\r', '\n' }, StringSplitOptions.RemoveEmptyEntries)
            .Select(s => s.Trim())
            .Where(s => !string.IsNullOrEmpty(s))
            .ToList();

        if (string.IsNullOrEmpty(reading))
        {
            await ShowDialogAsync("入力が不完全です", "読みを入力してください。");
            return null;
        }
        if (surfaces.Count == 0)
        {
            await ShowDialogAsync("入力が不完全です", "変換候補を 1 つ以上入力してください。");
            return null;
        }

        return new UserDictEntry
        {
            Reading = reading,
            Surfaces = surfaces,
        };
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
