; =============================================================================
; rakukan IME - Inno Setup Installer Script
; =============================================================================
; 使用方法:
;   1. Inno Setup 6 をインストール: https://jrsoftware.org/isinfo.php
;   2. ビルド済み成果物を dist\ フォルダに配置（後述の構成を参照）
;   3. このスクリプトを Inno Setup IDE か ISCC.exe でコンパイル
;
; dist\ フォルダの構成（このスクリプトと同じディレクトリに置く）:
;   dist\rakukan_tsf.dll
;   dist\rakukan_engine_cpu.dll
;   dist\rakukan_engine_vulkan.dll  (省略可)
;   dist\rakukan_engine_cuda.dll    (省略可)
;   dist\rakukan.dict
;   dist\config.toml
;   dist\models\                    (省略可)
;
; =============================================================================

#define MyAppName      "Rakukan IME"
#define MyAppVersion   "0.3.7"
#define MyAppPublisher "fukuyori"
#define MyAppURL       "https://github.com/fukuyori/rakukan"

[Setup]
AppId={{B7C4E2A1-3F8D-4C91-B5A0-D2E6F9183047}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}/issues
AppUpdatesURL={#MyAppURL}/releases

; インストール先は Code セクションで動的に決定する
; (管理者昇格時でも元ユーザーの LOCALAPPDATA\rakukan\ になるよう制御)
DefaultDirName={code:GetInstallDir}
DisableDirPage=yes
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes

; アンインストール情報
UninstallDisplayName={#MyAppName}
UninstallDisplayIcon={app}\rakukan.ico

; セットアップアイコン
SetupIconFile=dist\rakukan.ico

; 出力設定
OutputDir=output
OutputBaseFilename=rakukan-{#MyAppVersion}-setup
Compression=lzma2/ultra64
SolidCompression=yes
InternalCompressLevel=ultra64

; UI設定
WizardStyle=modern

; 管理者権限を要求 (regsvr32 に必要)
PrivilegesRequired=admin

; DLL使用中プロセスの終了ダイアログを抑制
; (regsvr32 /u で登録解除済みのため強制終了不要)
CloseApplications=no

; ログ
SetupLogging=yes

[Languages]
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"
Name: "english";  MessagesFile: "compiler:Default.isl"

[Messages]
japanese.WelcomeLabel1=rakukan IME セットアップへようこそ
japanese.WelcomeLabel2=このウィザードは rakukan IME をインストールします。%nWindows 日本語入力メソッドです。%n%nセットアップを続行するには [次へ] をクリックしてください。
japanese.FinishedHeadingLabel=rakukan IME のインストール完了
japanese.FinishedLabel=rakukan IME のインストールが完了しました。%n%n言語バーに表示されない場合は、一度サインアウトして再度ログインしてください。


[Files]
; ----- アイコン -----
Source: "dist\rakukan.ico"; DestDir: "{app}"; Flags: ignoreversion

; ----- TSF DLL -----
Source: "dist\rakukan_tsf.dll"; DestDir: "{app}"; Flags: ignoreversion

; ----- アイコン -----
Source: "dist\rakukan.ico"; DestDir: "{app}"; Flags: ignoreversion

; ----- Engine DLLs -----
Source: "dist\rakukan_engine_cpu.dll";    DestDir: "{app}"; Flags: ignoreversion
Source: "dist\rakukan_engine_vulkan.dll"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "dist\rakukan_engine_cuda.dll";   DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist

; ----- 辞書 -----
Source: "dist\rakukan.dict"; DestDir: "{code:GetRoamingConfigDir}\dict"; Flags: ignoreversion

; ----- デフォルト設定ファイル (既存は上書きしない) -----
; config.toml は %APPDATA%\rakukan\ に配置する（rakukan が読む場所）
Source: "dist\config.toml"; DestDir: "{code:GetRoamingConfigDir}"; Flags: onlyifdoesntexist uninsneveruninstall

; ----- LLM モデル (省略可) -----
Source: "dist\models\*"; DestDir: "{app}\models"; Flags: ignoreversion recursesubdirs skipifsourcedoesntexist


; ----- TIP 登録スクリプト -----
Source: "dist\register-tip.ps1";   DestDir: "{app}"; Flags: ignoreversion
Source: "dist\unregister-tip.ps1"; DestDir: "{app}"; Flags: ignoreversion

[Run]
; ----- COM/TSF 登録 -----
Filename: "{sys}\regsvr32.exe"; \
    Parameters: "/s ""{app}\rakukan_tsf.dll"""; \
    Flags: runhidden waituntilterminated; \
    StatusMsg: "TSF コンポーネントを登録中..."

; ----- キーボードリストへ追加 (WinUserLanguageList) -----
; postinstall: インストーラー終了後にユーザー権限で実行 (管理者権限では言語リストを正しく操作できない)
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; \
    Parameters: "-ExecutionPolicy Bypass -File ""{app}\register-tip.ps1"""; \
    Flags: postinstall runhidden waituntilterminated; \
    StatusMsg: "キーボードリストに rakukan を追加中..."; \
    Description: "キーボードリストに rakukan を追加する"

; ----- HKCU へ TIP キーをミラー (Windows 11 対応) -----
Filename: "{sys}\reg.exe"; \
    Parameters: "COPY ""HKLM\Software\Microsoft\CTF\TIP"" ""HKCU\Software\Microsoft\CTF\TIP"" /s /f"; \
    Flags: runhidden waituntilterminated; \
    StatusMsg: "入力メソッド設定を反映中..."

Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; \
    Flags: runhidden waituntilterminated; \

[UninstallRun]
; ----- キーボードリストから削除 -----
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; \
    Parameters: "-ExecutionPolicy Bypass -File ""{app}\unregister-tip.ps1"""; \
    Flags: runhidden waituntilterminated; \
    RunOnceId: "RemoveTIP"

; ----- COM 登録解除 -----
Filename: "{sys}\regsvr32.exe"; \
    Parameters: "/s /u ""{app}\rakukan_tsf.dll"""; \
    Flags: runhidden waituntilterminated; \
    RunOnceId: "UnregisterTSF"

; ----- HKCU の TIP キーを削除 -----
Filename: "{sys}\reg.exe"; \
    Parameters: "DELETE ""HKCU\Software\Microsoft\CTF\TIP"" /f"; \
    Flags: runhidden waituntilterminated; \
    RunOnceId: "CleanupHKCUTip"

[UninstallDelete]
Type: files;          Name: "{app}\rakukan_tsf.dll"
Type: files;          Name: "{app}\rakukan_engine_cpu.dll"
Type: files;          Name: "{app}\rakukan_engine_vulkan.dll"
Type: files;          Name: "{app}\rakukan_engine_cuda.dll"
Type: filesandordirs; Name: "{app}\dict"
; config.toml・models は残す（ユーザーデータ）

[Code]
// =========================================================================
// インストール先を「インストーラーを起動した元ユーザー」の
// LOCALAPPDATA\rakukan\ に固定する。
// PrivilegesRequired=admin で UAC 昇格すると {localappdata} が
// 管理者アカウントのパスになるため、USERPROFILE から組み立てる。
// =========================================================================

function GetUserLocalAppData(): String;
var
  UserProfile: String;
begin
  UserProfile := GetEnv('USERPROFILE');
  if UserProfile <> '' then
    Result := UserProfile + '\AppData\Local'
  else
    Result := GetEnv('LOCALAPPDATA');
end;

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

// DefaultDirName={code:GetInstallDir} から呼ばれる
function GetInstallDir(Param: String): String;
begin
  Result := GetUserLocalAppData() + '\rakukan';
end;

// 64-bit チェック・必須モジュール確認
function InitializeSetup(): Boolean;
var
  MissingDlls: String;
  HasCudaEngine: Boolean;
  HasVulkanEngine: Boolean;
  Msg: String;
begin
  // 64-bit チェック
  if not IsWin64 then begin
    MsgBox('rakukan IME は 64-bit Windows でのみ動作します。', mbError, MB_OK);
    Result := False;
    Exit;
  end;

  // Visual C++ Redistributable チェック（必須）
  MissingDlls := '';
  if not FileExists(ExpandConstant('{sys}\VCRUNTIME140.dll')) then
    MissingDlls := MissingDlls + '  - VCRUNTIME140.dll' + #13#10;
  if not FileExists(ExpandConstant('{sys}\MSVCP140.dll')) then
    MissingDlls := MissingDlls + '  - MSVCP140.dll' + #13#10;

  if MissingDlls <> '' then begin
    MsgBox(
      'Visual C++ 再頒布可能パッケージ (2015-2022) が見つかりません。' + #13#10 +
      '不足しているファイル:' + #13#10 + MissingDlls + #13#10 +
      '以下からインストールしてください:' + #13#10 +
      'https://aka.ms/vs/17/release/vc_redist.x64.exe',
      mbError, MB_OK);
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
      if MsgBox(Msg, mbConfirmation, MB_YESNO) = IDNO then begin
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
      if MsgBox(Msg, mbConfirmation, MB_YESNO) = IDNO then begin
        Result := False;
        Exit;
      end;
    end;
  end;

  Result := True;
end;

// バックアップ保存先（ロールバック用）
var
  BackupDll: String;
  InstallFailed: Boolean;

// インストール失敗時のロールバック処理
// ※ CurStepChanged より前に定義する必要あり（前方参照不可）
procedure InstallFail;
var
  ResultCode: Integer;
  InstallDir: String;
  NewDll: String;
begin
  InstallFailed := True;
  InstallDir := GetUserLocalAppData() + '\rakukan';
  NewDll     := InstallDir + '\rakukan_tsf.dll';

  // 新 DLL の登録を解除（中途半端に登録された可能性）
  if FileExists(NewDll) then
    Exec(ExpandConstant('{sys}\regsvr32.exe'),
         '/s /u "' + NewDll + '"',
         '', SW_HIDE, ewWaitUntilTerminated, ResultCode);

  // バックアップから旧 DLL を復元
  if FileExists(BackupDll) then begin
    CopyFile(BackupDll, NewDll, False);
    DeleteFile(BackupDll);
    // 旧 DLL を再登録
    Exec(ExpandConstant('{sys}\regsvr32.exe'),
         '/s "' + NewDll + '"',
         '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  end;

  // 失敗メッセージを表示
  MsgBox(
    'インストールに失敗しました。' + #13#10 + #13#10 +
    '前のバージョンに戻しました。引き続きご使用いただけます。' + #13#10 + #13#10 +
    '【原因】' + #13#10 +
    '  rakukan の DLL が使用中のためコピーできませんでした。' + #13#10 + #13#10 +
    '【再インストールの手順】' + #13#10 +
    '  1. 言語バーで rakukan 以外の IME（例：Microsoft IME）に切り替える' + #13#10 +
    '  2. ［スタート］→ アカウント名 → ［サインアウト］を選択する' + #13#10 +
    '  3. 再度サインインする' + #13#10 +
    '  4. インストーラーを再度実行する' + #13#10 + #13#10 +
    '※ PC の再起動は不要です。サインアウト→サインインで解決します。',
    mbError, MB_OK);
end;

// インストール前に旧 DLL をバックアップ＆登録解除する
// インストール失敗時のロールバックに使用
procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
  OldDll: String;
  InstallDir: String;
begin
  InstallDir := GetUserLocalAppData() + '\rakukan';
  OldDll     := InstallDir + '\rakukan_tsf.dll';

  if CurStep = ssInstall then begin
    InstallFailed := False;

    if FileExists(OldDll) then begin
      // 旧 DLL をバックアップ
      BackupDll := InstallDir + '\rakukan_tsf.dll.backup';
      CopyFile(OldDll, BackupDll, False);

      // 旧 DLL を登録解除
      Exec(ExpandConstant('{sys}\regsvr32.exe'),
           '/s /u "' + OldDll + '"',
           '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
      Sleep(1500);
    end;
  end;

  if CurStep = ssPostInstall then begin
    // インストール後に TSF DLL が存在するか確認
    // DLL が消えていれば確実に失敗
    if not FileExists(InstallDir + '\rakukan_tsf.dll') then begin
      InstallFail;
    end;
  end;

  if CurStep = ssDone then begin
    // インストール成功時：バックアップを削除
    if (not InstallFailed) and FileExists(BackupDll) then
      DeleteFile(BackupDll);
  end;
end;

// Inno Setup の標準失敗コールバック
procedure CancelButtonClick(CurPageID: Integer; var Cancel, Confirm: Boolean);
begin
  // キャンセル時もロールバックを実行
  if InstallFailed then begin
    Confirm := False;
  end;
end;


// インストール前チェック：旧 DLL がロックされていないか確認
// wpSelectTasks（追加タスク選択）ページ表示時に実行する
var
  DllLockChecked: Boolean; // 重複チェック防止フラグ

procedure CheckDllLock;
var
  OldDll: String;
  TempFile: String;
  Attempts: Integer;
  Locked: Boolean;
  ResultCode: Integer;
begin
  if DllLockChecked then Exit;
  DllLockChecked := True;

  OldDll := GetUserLocalAppData() + '\rakukan\rakukan_tsf.dll';
  if not FileExists(OldDll) then Exit; // 初回インストール：チェック不要

  // regsvr32 /u で登録解除を先行実行
  Exec(ExpandConstant('{sys}\regsvr32.exe'),
       '/s /u "' + OldDll + '"',
       '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Sleep(1500);

  // リネームでロック確認（最大3回）
  TempFile := OldDll + '.bak_setup';
  Locked := True;
  for Attempts := 1 to 3 do begin
    if RenameFile(OldDll, TempFile) then begin
      RenameFile(TempFile, OldDll);
      Locked := False;
      Break;
    end;
    Sleep(1000);
  end;

  if Locked then begin
    MsgBox(
      'rakukan の DLL が使用中のため、このままではインストールできません。' + #13#10 + #13#10 +
      '【対処方法】' + #13#10 +
      '  1. 言語バーで rakukan 以外の IME（例：Microsoft IME）に切り替える' + #13#10 +
      '  2. ［スタート］→ アカウント名 → ［サインアウト］を選択する' + #13#10 +
      '  3. 再度サインインしてからインストーラーを再実行する' + #13#10 + #13#10 +
      '※ PC の再起動は不要です。サインアウト→サインインで解決します。' + #13#10 + #13#10 +
      'このままインストールを続行することもできますが、' + #13#10 +
      'DLL の上書きに失敗する可能性があります。',
      mbError, MB_OK);
  end;
end;

procedure CurPageChanged(CurPageID: Integer);
begin
  if CurPageID = wpSelectTasks then
    CheckDllLock;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  NeedsRestart := False;
  // DLL ロックチェックは wpSelectTasks ページで実施済み
end;
