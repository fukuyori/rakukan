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
;   dist\download-skk.ps1
;   dist\models\                    (省略可)
;
; =============================================================================

#define MyAppName      "Rakukan IME"
#define MyAppVersion   "0.3.1"
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

[Tasks]
Name: "downloadskk";   Description: "SKK-JISYO.L をダウンロードする（推奨、約 10MB）"; GroupDescription: "オプション:"

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
Source: "dist\rakukan.dict"; DestDir: "{app}\dict"; Flags: ignoreversion

; ----- デフォルト設定ファイル (既存は上書きしない) -----
; config.toml は %APPDATA%\rakukan\ に配置する（rakukan が読む場所）
Source: "dist\config.toml"; DestDir: "{code:GetRoamingConfigDir}"; Flags: onlyifdoesntexist uninsneveruninstall

; ----- LLM モデル (省略可) -----
Source: "dist\models\*"; DestDir: "{app}\models"; Flags: ignoreversion recursesubdirs skipifsourcedoesntexist

; ----- SKK ダウンロードスクリプト -----
Source: "dist\download-skk.ps1"; DestDir: "{app}"; Flags: ignoreversion

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

; ----- SKK-JISYO.L ダウンロード -----
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; \
    Parameters: "-ExecutionPolicy Bypass -File ""{app}\download-skk.ps1"""; \
    Flags: runhidden waituntilterminated; \
    StatusMsg: "SKK-JISYO.L をダウンロード中..."; \
    Tasks: downloadskk

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
Type: files;          Name: "{app}\download-skk.ps1"
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

// 64-bit チェック
function InitializeSetup(): Boolean;
begin
  if not IsWin64 then begin
    MsgBox('rakukan IME は 64-bit Windows でのみ動作します。', mbError, MB_OK);
    Result := False;
    Exit;
  end;
  Result := True;
end;

// インストール前に旧 DLL を登録解除する
procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
  OldDll: String;
begin
  if CurStep = ssInstall then begin
    OldDll := GetUserLocalAppData() + '\rakukan\rakukan_tsf.dll';
    if FileExists(OldDll) then begin
      Exec(ExpandConstant('{sys}\regsvr32.exe'),
           '/s /u "' + OldDll + '"',
           '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
      // DLL ロック解放を待つ
      Sleep(1000);
    end;
  end;
end;
