using System.Runtime.InteropServices;
using Microsoft.UI.Xaml;
using Microsoft.Windows.AppLifecycle;

namespace Rakukan.Settings.WinUI;

public partial class App : Application
{
    private const int SW_RESTORE = 9;

    private Window? _window;

    public App()
    {
        InitializeComponent();
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        _window = new MainWindow();
        _window.Activate();

        // 2 つ目のプロセス（Program.Main で転送）からのアクティベーションを受け取り、
        // 新しいウインドウを開く代わりに既存ウインドウを前面に出す。
        AppInstance.GetCurrent().Activated += OnRedirectedActivation;
    }

    private void OnRedirectedActivation(object? sender, AppActivationArguments e)
    {
        // このイベントは UI スレッド以外で発火するため、DispatcherQueue 経由で処理する
        _window?.DispatcherQueue.TryEnqueue(BringWindowToFront);
    }

    private void BringWindowToFront()
    {
        if (_window is null)
        {
            return;
        }

        var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(_window);
        if (IsIconic(hwnd))
        {
            ShowWindow(hwnd, SW_RESTORE);
        }

        SetForegroundWindow(hwnd);
        _window.Activate();
    }

    [DllImport("user32.dll")]
    private static extern bool IsIconic(IntPtr hWnd);

    [DllImport("user32.dll")]
    private static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

    [DllImport("user32.dll")]
    private static extern bool SetForegroundWindow(IntPtr hWnd);
}
