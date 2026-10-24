; =============================================================================
; rakukan IME - Inno Setup Installer Script
; =============================================================================
; 使用方法:
;   1. Inno Setup 6 をインストール: https://jrsoftware.org/isinfo.php
;   2. scripts\build-installer.ps1 でビルドの出力から dist\ を組み立て、このスクリプトを ISCC.exe でコンパイルする
;      （cargo make install は署名なしで作ってサイレント実行、cargo make package は署名付きで作る）
;   モデルは同梱しない（engine-host が初回の変換で %ProgramData%\rakukan\models に取得する）。
;
; 方式（docs/Installer_Redesign_Plan.md の 2 章・4.1）:
;   - 配置は %ProgramFiles%\rakukan（マシン単位、管理者）。64 ビットのモードで動かす。
;   - 旧ホストは止めない。使用中のファイルは次の Windows の起動時に置き換え・削除する
;     （restartreplace / uninsrestartdelete）。インストール・アンインストールの完了には再起動が要る。
;   - TSF の登録は [Run] の regsvr32 で行い、成否は登録の状態（レジストリ）で決める。
;   - キーボードの一覧への追加は、インストーラーを起動したユーザーの権限で行う。
;     入らなかった場合は、サインインのときにトレイが補う（版ごとに 1 回）。
;   - 旧配置（%LOCALAPPDATA%\rakukan）があれば、登録を解除し、旧版が置いたファイルを消す
;     （使用中のものは次の Windows の起動時に消す）。
;
; dist\ フォルダの構成（このスクリプトと同じディレクトリに置く）:
;   dist\rakukan_tsf.dll
;   dist\rakukan_engine_cpu.dll
;   dist\rakukan_engine_vulkan.dll  (省略可)
;   dist\rakukan_engine_cuda.dll    (省略可)
;   dist\rakukan-engine-host.exe
;   dist\rakukan-tray.exe
;   dist\settings-ui\
;   dist\rakukan.dict
;   dist\config.toml
;   dist\register-tip.ps1 / unregister-tip.ps1
;   dist\LICENSE / LICENSE_AGREEMENT.txt / NOTICE / THIRD_PARTY_LICENSES.md
;
; 終了コード: 登録の状態を確かめられなかったときは 20（ExitCodeRegistrationFailed）。
; =============================================================================

#define MyAppName      "Rakukan IME"
#define MyAppVersion   "0.13.0"
#define MyAppPublisher "fukuyori"
#define MyAppURL       "https://github.com/fukuyori/rakukan"
#define TsfClsid       "{C0DDF8B0-1F1E-4C2D-A9E3-5F7B8D6E2A4C}"

[Setup]
AppId={{B7C4E2A1-3F8D-4C91-B5A0-D2E6F9183047}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}/issues
AppUpdatesURL={#MyAppURL}/releases

; 64 ビットのモード（{sys} が System32、{autopf} が Program Files を指す）
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

DefaultDirName={autopf}\rakukan
DisableDirPage=yes
; 同じ AppId の旧版は %LOCALAPPDATA%\rakukan に入っている。前回のインストール先を使い回すと
; そこへ入れてしまうので使わない（旧配置は ssInstall で片付ける）
UsePreviousAppDir=no
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes

; アンインストール情報
UninstallDisplayName={#MyAppName}
UninstallDisplayIcon={app}\rakukan.ico

; セットアップアイコン
SetupIconFile=dist\rakukan.ico

; 出力設定
OutputDir=output
; 使用許諾の画面。build-installer.ps1 が docs\licenses から作る（設定アプリが同梱する Microsoft の部品の条項を含む）
LicenseFile=dist\LICENSE_AGREEMENT.txt
OutputBaseFilename=rakukan-{#MyAppVersion}-setup
Compression=lzma2/ultra64
SolidCompression=yes
InternalCompressLevel=ultra64

; UI設定
WizardStyle=modern

; 管理者権限を要求（HKLM への登録、restartreplace / uninsrestartdelete に必要）
PrivilegesRequired=admin

; 使用中のファイルを持つアプリは閉じない（旧ホストも止めない）。
; 置き換えられなかったファイルは、次の Windows の起動時に置き換える。
CloseApplications=no

; インストール・アンインストールの完了には再起動が要る
AlwaysRestart=yes
UninstallRestartComputer=yes

; ログ
SetupLogging=yes

; コード署名 (scripts\build-installer.ps1 -Sign から /DSIGN + /Srakukan=... で有効化)
; SignedUninstaller=yes により、インストール先に置かれる unins000.exe にも署名が付く。
#ifdef SIGN
SignTool=rakukan
SignedUninstaller=yes
#endif

[Languages]
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"
Name: "english";  MessagesFile: "compiler:Default.isl"

[Messages]
japanese.WelcomeLabel1=rakukan IME セットアップへようこそ
japanese.WelcomeLabel2=このウィザードは rakukan IME をインストールします。%nWindows 日本語入力メソッドです。%n%nセットアップを続行するには [次へ] をクリックしてください。
japanese.FinishedHeadingLabel=rakukan IME のインストール完了
; 再起動が要るときの完了の画面（FinishedRestartLabel）と、無人の実行での確認（FinishedRestartMessage）
japanese.FinishedRestartLabel=インストールを完了するには、PC を再起動してください。再起動するまでは、Rakukan による日本語入力ができないことがあります。%n%n今すぐ再起動しますか?
japanese.FinishedRestartMessage=インストールを完了するには、PC を再起動してください。再起動するまでは、Rakukan による日本語入力ができないことがあります。%n%n今すぐ再起動しますか?
; /NORESTART で再起動を求めない場合の完了の画面
japanese.FinishedLabelNoIcons=インストールを完了するには、PC を再起動してください。再起動するまでは、Rakukan による日本語入力ができないことがあります。
english.FinishedRestartLabel=To complete the installation, please restart your PC. Until you restart, Japanese input with Rakukan may not be available.%n%nWould you like to restart now?
english.FinishedRestartMessage=To complete the installation, please restart your PC. Until you restart, Japanese input with Rakukan may not be available.%n%nWould you like to restart now?
english.FinishedLabelNoIcons=To complete the installation, please restart your PC. Until you restart, Japanese input with Rakukan may not be available.

[Dirs]
; 全ユーザーで共有するモデルの置き場所。どのユーザーのホストも取得・読み込みできるようにする
Name: "{commonappdata}\rakukan\models"; Permissions: users-modify; Flags: uninsneveruninstall

[Files]
; 使用中のファイルは次の Windows の起動時に置き換える（restartreplace）。
; アンインストールで消せないものは次の起動時に消す（uninsrestartdelete）。

; ----- アイコン -----
Source: "dist\rakukan.ico"; DestDir: "{app}"; Flags: ignoreversion restartreplace uninsrestartdelete

; ----- TSF DLL -----
Source: "dist\rakukan_tsf.dll"; DestDir: "{app}"; Flags: ignoreversion restartreplace uninsrestartdelete

; ----- Engine DLLs -----
Source: "dist\rakukan_engine_cpu.dll";    DestDir: "{app}"; Flags: ignoreversion restartreplace uninsrestartdelete
Source: "dist\rakukan_engine_vulkan.dll"; DestDir: "{app}"; Flags: ignoreversion restartreplace uninsrestartdelete skipifsourcedoesntexist
Source: "dist\rakukan_engine_cuda.dll";   DestDir: "{app}"; Flags: ignoreversion restartreplace uninsrestartdelete skipifsourcedoesntexist

; ----- Engine Host (out-of-process) -----
; TSF DLL からは engine DLL をロードせず、このホストプロセスへ Named Pipe で RPC する。
; これにより Zoom / Dropbox 等のホストアプリに llama.cpp を持ち込まない。
Source: "dist\rakukan-engine-host.exe"; DestDir: "{app}"; Flags: ignoreversion restartreplace uninsrestartdelete

; ----- トレイ（ログの掃除・キーボードの一覧の補い） -----
Source: "dist\rakukan-tray.exe"; DestDir: "{app}"; Flags: ignoreversion restartreplace uninsrestartdelete

; ----- Settings GUI (WinUI 3) -----
Source: "dist\settings-ui\*"; DestDir: "{app}\settings-ui"; Flags: ignoreversion restartreplace uninsrestartdelete recursesubdirs createallsubdirs skipifsourcedoesntexist

; ----- 辞書 -----
Source: "dist\rakukan.dict"; DestDir: "{app}\dict"; Flags: ignoreversion restartreplace uninsrestartdelete
Source: "dist\rakukan.dict.build.json"; DestDir: "{app}\dict"; Flags: ignoreversion restartreplace uninsrestartdelete skipifsourcedoesntexist

; ----- デフォルト設定ファイル (既存は上書きしない) -----
; config.toml は %APPDATA%\rakukan\ に配置する（rakukan が読む場所）
Source: "dist\config.toml"; DestDir: "{code:GetRoamingConfigDir}"; Flags: onlyifdoesntexist uninsneveruninstall

; ----- TIP 登録スクリプト -----
Source: "dist\register-tip.ps1";   DestDir: "{app}"; Flags: ignoreversion restartreplace uninsrestartdelete
Source: "dist\unregister-tip.ps1"; DestDir: "{app}"; Flags: ignoreversion restartreplace uninsrestartdelete

; ----- ライセンス・帰属表示 -----
Source: "dist\LICENSE";                 DestDir: "{app}"; Flags: ignoreversion restartreplace uninsrestartdelete
Source: "dist\LICENSE_AGREEMENT.txt";   DestDir: "{app}"; Flags: ignoreversion restartreplace uninsrestartdelete
Source: "dist\NOTICE";                  DestDir: "{app}"; Flags: ignoreversion restartreplace uninsrestartdelete
Source: "dist\THIRD_PARTY_LICENSES.md"; DestDir: "{app}"; Flags: ignoreversion restartreplace uninsrestartdelete

[Registry]
; トレイを、どのユーザーのサインインでも起動する（HKLM の Run）。アンインストールで消す
Root: HKLM; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "rakukan-tray"; ValueData: """{app}\rakukan-tray.exe"""; Flags: uninsdeletevalue

[Run]
; postinstall の項目は再起動が要るときに実行されないので使わない。
; ここの項目はインストールの終わり（完了の画面の前）に、書いた順に実行される。

; ----- COM/TSF 登録 -----
; 成否は終了コードでは決めず、CurStepChanged(ssPostInstall) で登録の状態を確かめる
Filename: "{sys}\regsvr32.exe"; \
    Parameters: "/s ""{app}\rakukan_tsf.dll"""; \
    Flags: runhidden waituntilterminated; \
    StatusMsg: "TSF コンポーネントを登録中..."

; ----- キーボードリストへ追加（HKCU への TIP の写しを含む） -----
; インストーラーを起動したユーザーの権限で行う。昇格した端末から起動した場合は効かず、
; そのときはトレイがサインインのときに補う。
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; \
    Parameters: "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File ""{app}\register-tip.ps1"""; \
    Flags: runasoriginaluser runhidden waituntilterminated; \
    StatusMsg: "キーボードリストに rakukan を追加中..."

; ----- トレイの起動 -----
Filename: "{app}\rakukan-tray.exe"; \
    Flags: runasoriginaluser nowait

[UninstallRun]
; ----- キーボードリストから削除（HKCU への TIP の写しを含む） -----
; [UninstallRun] では runasoriginaluser を使えないので、アンインストーラーの権限で実行する
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; \
    Parameters: "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File ""{app}\unregister-tip.ps1"""; \
    Flags: runhidden waituntilterminated; \
    RunOnceId: "RemoveTIP"

; ----- COM 登録解除 -----
Filename: "{sys}\regsvr32.exe"; \
    Parameters: "/s /u ""{app}\rakukan_tsf.dll"""; \
    Flags: runhidden waituntilterminated; \
    RunOnceId: "UnregisterTSF"

[UninstallDelete]
Type: filesandordirs; Name: "{app}\settings-ui"
Type: filesandordirs; Name: "{app}\dict"
; config.toml・models は残す（ユーザーデータ）

[Code]
const
  // 登録の状態を確かめられなかったときの終了コード（Inno Setup の既定の終了コードと重ならない値）
  ExitCodeRegistrationFailed = 20;
  TsfClsid = '{#TsfClsid}';

var
  RegistrationFailed: Boolean;

// config.toml の配置先: %APPDATA%\rakukan\
// rakukan は APPDATA (Roaming) の config.toml を読む
function GetRoamingConfigDir(Param: String): String;
var
  UserProfile: String;
  RoamingAppData: String;
begin
  UserProfile := GetEnv('USERPROFILE');
  if UserProfile <> '' then
    RoamingAppData := UserProfile + '\AppData\Roaming'
  else
    RoamingAppData := GetEnv('APPDATA');
  Result := RoamingAppData + '\rakukan';
  // ディレクトリが存在しない場合は作成
  if not DirExists(Result) then
    CreateDir(Result);
end;

// 必須モジュール確認
function InitializeSetup(): Boolean;
var
  MissingDlls: String;
  HasCudaEngine: Boolean;
  HasVulkanEngine: Boolean;
  Msg: String;
begin
  // Visual C++ Redistributable チェック（必須）
  MissingDlls := '';
  if not FileExists(ExpandConstant('{sys}\VCRUNTIME140.dll')) then
    MissingDlls := MissingDlls + '  - VCRUNTIME140.dll' + #13#10;
  if not FileExists(ExpandConstant('{sys}\MSVCP140.dll')) then
    MissingDlls := MissingDlls + '  - MSVCP140.dll' + #13#10;

  if MissingDlls <> '' then begin
    SuppressibleMsgBox(
      'Visual C++ 再頒布可能パッケージ (2015-2022) が見つかりません。' + #13#10 +
      '不足しているファイル:' + #13#10 + MissingDlls + #13#10 +
      '以下からインストールしてください:' + #13#10 +
      'https://aka.ms/vs/17/release/vc_redist.x64.exe',
      mbError, MB_OK, IDOK);
    Result := False;
    Exit;
  end;

  // CUDA エンジン DLL が同梱されている場合、CUDA 依存 DLL を確認
  HasCudaEngine := FileExists(ExpandConstant('{src}\dist\rakukan_engine_cuda.dll'));
  if HasCudaEngine then begin
    MissingDlls := '';
    if not FileExists(ExpandConstant('{sys}\cublas64_13.dll')) then
      MissingDlls := MissingDlls + '  - cublas64_13.dll' + #13#10;
    if not FileExists(ExpandConstant('{sys}\nvcudart_hybrid64.dll')) then
      MissingDlls := MissingDlls + '  - nvcudart_hybrid64.dll' + #13#10;

    if MissingDlls <> '' then begin
      Msg :=
        'CUDA バックエンドに必要な DLL が C:\Windows\System32 に見つかりません:' + #13#10 +
        MissingDlls + #13#10 +
        'CUDA を使用しない場合はそのままインストールを続行できます。' + #13#10 +
        'CUDA を使用する場合は、インストール後に以下の手順で DLL をコピーしてください:' + #13#10 +
        '  CUDA Toolkit 13.x の bin\x64\ から' + #13#10 +
        '  C:\Windows\System32 へコピー（管理者権限が必要）' + #13#10 + #13#10 +
        'インストールを続行しますか？';
      if SuppressibleMsgBox(Msg, mbConfirmation, MB_YESNO, IDYES) = IDNO then begin
        Result := False;
        Exit;
      end;
    end;
  end;

  // Vulkan エンジン DLL が同梱されている場合、Vulkan ランタイムを確認
  HasVulkanEngine := FileExists(ExpandConstant('{src}\dist\rakukan_engine_vulkan.dll'));
  if HasVulkanEngine then begin
    if not FileExists(ExpandConstant('{sys}\vulkan-1.dll')) then begin
      Msg :=
        'Vulkan バックエンドに必要な vulkan-1.dll が見つかりません。' + #13#10 +
        'Vulkan を使用しない場合はそのままインストールを続行できます。' + #13#10 + #13#10 +
        'インストールを続行しますか？';
      if SuppressibleMsgBox(Msg, mbConfirmation, MB_YESNO, IDYES) = IDNO then begin
        Result := False;
        Exit;
      end;
    end;
  end;

  Result := True;
end;

// 登録の状態: CLSID の InprocServer32 がインストール先の TSF DLL を指し、TIP のキーがある
function TsfRegistered(): Boolean;
var
  Path: String;
begin
  Result :=
    RegQueryStringValue(HKLM, 'Software\Classes\CLSID\' + TsfClsid + '\InprocServer32', '', Path) and
    (CompareText(Path, ExpandConstant('{app}\rakukan_tsf.dll')) = 0) and
    RegKeyExists(HKLM, 'Software\Microsoft\CTF\TIP\' + TsfClsid);
  if not Result then
    Log('TSF registration check failed: InprocServer32=' + Path);
end;

// ---------------------------------------------------------------------------
// 旧配置（%LOCALAPPDATA%\rakukan、0.13.0 までのインストーラーと install.ps1）からの移行
// （docs/Installer_Redesign_Plan.md の Step 3）
//
// 旧配置の TSF DLL の登録を解除し、旧版が置いたファイルを消す。使用中のもの（動いている
// アプリの TSF DLL、旧ホスト、旧トレイ、設定アプリ）は、次の Windows の起動時に消す予約をする。
// 消すのは旧版が置いた名前のファイルと settings-ui\・dict\ だけで、ログ・etw\・設定のファイル
// など、それ以外は残す。対象はインストーラーを実行したユーザーの %LOCALAPPDATA%。
// ---------------------------------------------------------------------------

function OldInstallDir(): String;
begin
  Result := ExpandConstant('{localappdata}\rakukan');
end;

// 消す。消せなければ次の Windows の起動時に消す予約をする
procedure DeleteNowOrAtRestart(const Path: String);
begin
  if DeleteFile(Path) then begin
    Log('migration: removed ' + Path);
    Exit;
  end;
  try
    RestartReplace(Path, '');
    Log('migration: delete at restart ' + Path);
  except
    Log('migration: cannot remove ' + Path + ': ' + GetExceptionMessage);
  end;
end;

// フォルダーの中身を消してから、フォルダーを消す。予約は中身が先、フォルダーが後になる
// （起動時の削除は登録した順に行われ、フォルダーは空のときだけ消える）
procedure DeleteTreeNowOrAtRestart(const Dir: String);
var
  FindRec: TFindRec;
begin
  if not DirExists(Dir) then
    Exit;
  if FindFirst(Dir + '\*', FindRec) then begin
    try
      repeat
        if (FindRec.Name <> '.') and (FindRec.Name <> '..') then begin
          if (FindRec.Attributes and FILE_ATTRIBUTE_DIRECTORY) <> 0 then
            DeleteTreeNowOrAtRestart(Dir + '\' + FindRec.Name)
          else
            DeleteNowOrAtRestart(Dir + '\' + FindRec.Name);
        end;
      until not FindNext(FindRec);
    finally
      FindClose(FindRec);
    end;
  end;
  if RemoveDir(Dir) then
    Log('migration: removed ' + Dir)
  else begin
    try
      RestartReplace(Dir, '');
      Log('migration: delete at restart ' + Dir);
    except
      Log('migration: cannot remove ' + Dir + ': ' + GetExceptionMessage);
    end;
  end;
end;

// 0.13.0 までのインストーラーは 32 ビットのモードで動き、アンインストール情報を 32 ビット側
// （WOW6432Node）に書いた。64 ビットのモードのこのインストーラーはそれを引き継がず、旧版の
// アンインストーラーは移行で消えるので、「アプリ」の一覧に削除できない旧版が残る。
// 旧配置を指しているものだけを消す（旧配置のフォルダーが無くても行う）。
procedure RemoveOldUninstallEntry();
var
  Key, AppPath: String;
begin
  // [Setup] の AppId と同じ GUID
  Key := 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{B7C4E2A1-3F8D-4C91-B5A0-D2E6F9183047}_is1';
  if not RegQueryStringValue(HKLM32, Key, 'Inno Setup: App Path', AppPath) then
    Exit;
  if CompareText(RemoveBackslashUnlessRoot(AppPath), OldInstallDir()) <> 0 then begin
    Log('migration: kept 32-bit uninstall entry (App Path=' + AppPath + ')');
    Exit;
  end;
  if RegDeleteKeyIncludingSubkeys(HKLM32, Key) then
    Log('migration: removed old uninstall entry HKLM32\' + Key + ' (App Path=' + AppPath + ')')
  else
    Log('migration: could not remove old uninstall entry HKLM32\' + Key);
end;

procedure MigrateOldInstall();
var
  OldDir, OldDll, RunValue: String;
  ResultCode, I: Integer;
  Names: TArrayOfString;
begin
  OldDir := OldInstallDir();
  if (CompareText(OldDir, ExpandConstant('{app}')) = 0) or not DirExists(OldDir) then
    Exit;

  OldDll := OldDir + '\rakukan_tsf.dll';
  if FileExists(OldDll) then begin
    // 新しい DLL の登録（[Run]）より前に行う。CLSID が同じなので、後で解除すると新しい登録まで消える。
    // 旧版の DLL では解除が失敗・異常終了することがある（0.12.0 より前の DllMain の作り）ので、
    // 結果にかかわらず続ける。
    if Exec(ExpandConstant('{sys}\regsvr32.exe'), '/s /u "' + OldDll + '"',
            '', SW_HIDE, ewWaitUntilTerminated, ResultCode) then
      Log(Format('migration: old TSF DLL unregister %s: exit code %d (ignored)', [OldDll, ResultCode]))
    else
      Log('migration: could not run regsvr32 for ' + OldDll + ' (ignored)');
  end;

  // install.ps1 が HKCU の Run に書いたトレイ（旧配置を指すものだけ）。今は HKLM の Run から起動する
  if RegQueryStringValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'rakukan-tray', RunValue) and
     (Pos(Lowercase(OldDir + '\'), Lowercase(RunValue)) > 0) then begin
    if RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'rakukan-tray') then
      Log('migration: removed HKCU Run rakukan-tray (' + RunValue + ')');
  end;

  // 旧版のインストーラーと install.ps1 が置いたファイル
  Names := [
    'rakukan_tsf.dll', 'rakukan_tsf.dll.backup',
    'rakukan_engine_cpu.dll', 'rakukan_engine_vulkan.dll', 'rakukan_engine_cuda.dll',
    'rakukan-engine-host.exe', 'rakukan-tray.exe', 'rakukan-tray.exe.new',
    'rakukan-dict-builder.exe', 'rakukan-settings.exe', 'rakukan.ico',
    'register-tip.ps1', 'unregister-tip.ps1', 'registered.txt',
    'LICENSE', 'LICENSE_AGREEMENT.txt', 'NOTICE', 'THIRD_PARTY_LICENSES.md',
    'unins000.exe', 'unins000.dat', 'unins000.msg'];
  for I := 0 to GetArrayLength(Names) - 1 do begin
    if FileExists(OldDir + '\' + Names[I]) then
      DeleteNowOrAtRestart(OldDir + '\' + Names[I]);
  end;
  DeleteTreeNowOrAtRestart(OldDir + '\settings-ui');
  DeleteTreeNowOrAtRestart(OldDir + '\dict');
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
  OldDll: String;
begin
  if CurStep = ssInstall then begin
    MigrateOldInstall();
    RemoveOldUninstallEntry();

    // 前の版の登録を解除しておく。旧版の DLL では解除が失敗・異常終了することがある
    // （0.12.0 より前の DllMain の作り）ので、結果にかかわらず続ける。
    OldDll := ExpandConstant('{app}\rakukan_tsf.dll');
    if FileExists(OldDll) then begin
      if Exec(ExpandConstant('{sys}\regsvr32.exe'), '/s /u "' + OldDll + '"',
              '', SW_HIDE, ewWaitUntilTerminated, ResultCode) then
        Log(Format('old TSF DLL unregister: exit code %d (ignored)', [ResultCode]))
      else
        Log('old TSF DLL unregister: could not run regsvr32 (ignored)');
    end;
  end;

  if CurStep = ssPostInstall then begin
    // [Run] の regsvr32 はこの前に終わっている。終了コード 0 は登録の成功を示さないので、
    // レジストリの状態で確かめる。
    if not TsfRegistered() then begin
      RegistrationFailed := True;
      SuppressibleMsgBox(
        'rakukan の登録を確かめられませんでした。' + #13#10 +
        'PC を再起動してから、もう一度インストールしてください。' + #13#10 +
        '直らない場合は、セットアップのログ（%TEMP%\Setup Log *.txt）を添えて報告してください。',
        mbError, MB_OK, IDOK);
    end;
  end;
end;

function GetCustomSetupExitCode(): Integer;
begin
  if RegistrationFailed then
    Result := ExitCodeRegistrationFailed
  else
    Result := 0;
end;
