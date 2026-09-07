using System.Runtime.InteropServices;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.Windows.AppLifecycle;

namespace Rakukan.Settings.WinUI;

/// <summary>
/// 設定アプリを単一インスタンスにするためのカスタムエントリポイント
/// （csproj の DISABLE_XAML_GENERATED_MAIN で XAML 生成の Main を無効化している）。
/// 2 つ目以降のプロセスは既存インスタンスへアクティベーションを転送してすぐ終了する。
/// </summary>
public static class Program
{
    private const string InstanceKey = "rakukan-settings";

    [STAThread]
    private static void Main(string[] args)
    {
        WinRT.ComWrappersSupport.InitializeComWrappers();

        var mainInstance = AppInstance.FindOrRegisterForKey(InstanceKey);
        if (!mainInstance.IsCurrent)
        {
            // 既存インスタンスが自分のウインドウを前面に出せるよう、フォアグラウンド権限を渡す
            AllowSetForegroundWindow(mainInstance.ProcessId);
            RedirectActivationTo(AppInstance.GetCurrent().GetActivatedEventArgs(), mainInstance);
            return;
        }

        Application.Start(p =>
        {
            var context = new DispatcherQueueSynchronizationContext(DispatcherQueue.GetForCurrentThread());
            SynchronizationContext.SetSynchronizationContext(context);
            new App();
        });
    }

    // STA スレッドで RedirectActivationToAsync を直接 Wait すると COM 呼び出しがデッドロックし得るため、
    // 別スレッドで実行し、CoWaitForMultipleObjects でメッセージを処理しながら完了を待つ
    // （Windows App SDK の AppLifecycle サンプルと同じ手順）。
    private static void RedirectActivationTo(AppActivationArguments args, AppInstance keyInstance)
    {
        var redirectEventHandle = CreateEvent(IntPtr.Zero, true, false, null);
        Task.Run(() =>
        {
            keyInstance.RedirectActivationToAsync(args).AsTask().Wait();
            SetEvent(redirectEventHandle);
        });
        _ = CoWaitForMultipleObjects(0, 0xFFFFFFFF, 1, [redirectEventHandle], out _);
        CloseHandle(redirectEventHandle);
    }

    [DllImport("user32.dll")]
    private static extern bool AllowSetForegroundWindow(uint dwProcessId);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr CreateEvent(IntPtr lpEventAttributes, bool bManualReset, bool bInitialState, string? lpName);

    [DllImport("kernel32.dll")]
    private static extern bool SetEvent(IntPtr hEvent);

    [DllImport("kernel32.dll")]
    private static extern bool CloseHandle(IntPtr hObject);

    [DllImport("ole32.dll")]
    private static extern uint CoWaitForMultipleObjects(uint dwFlags, uint dwMilliseconds, uint nHandles, IntPtr[] pHandles, out uint dwIndex);
}
