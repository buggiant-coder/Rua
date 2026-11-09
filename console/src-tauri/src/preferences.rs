use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::Manager;

const PREFERENCES_VERSION: u8 = 1;
const PREFERENCES_FILE: &str = "desktop-preferences.json";
static PREFERENCES_LOCK: Mutex<()> = Mutex::new(());
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DesktopLocale {
    #[serde(rename = "en")]
    English,
    #[default]
    #[serde(rename = "zh")]
    SimplifiedChinese,
    #[serde(rename = "ja")]
    Japanese,
    #[serde(rename = "ru")]
    Russian,
}

impl DesktopLocale {
    pub fn from_tag(tag: &str) -> Self {
        match tag
            .split(['-', '_'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "zh" => Self::SimplifiedChinese,
            "ja" => Self::Japanese,
            "ru" => Self::Russian,
            _ => Self::English,
        }
    }

    pub const fn tag(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::SimplifiedChinese => "zh",
            Self::Japanese => "ja",
            Self::Russian => "ru",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreferences {
    #[serde(default = "preferences_version")]
    version: u8,
    #[serde(default)]
    rua_root: Option<PathBuf>,
    #[serde(default)]
    locale: DesktopLocale,
    #[serde(default = "default_confirm_before_quit")]
    confirm_before_quit: bool,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

impl Default for DesktopPreferences {
    fn default() -> Self {
        Self {
            version: PREFERENCES_VERSION,
            rua_root: None,
            locale: DesktopLocale::default(),
            confirm_before_quit: true,
            extra: BTreeMap::new(),
        }
    }
}

impl DesktopPreferences {
    pub fn rua_root(&self) -> Option<&Path> {
        self.rua_root.as_deref()
    }

    pub fn confirm_before_quit(&self) -> bool {
        self.confirm_before_quit
    }

    pub fn locale(&self) -> DesktopLocale {
        self.locale
    }
}

#[derive(Debug, Clone)]
pub struct DesktopPreferencesStore {
    path: PathBuf,
}

impl DesktopPreferencesStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn for_app(app: &tauri::AppHandle) -> Result<Self, String> {
        let directory = app
            .path()
            .app_config_dir()
            .map_err(|error| format!("failed to resolve Desktop settings directory: {error}"))?;
        Ok(Self::new(directory.join(PREFERENCES_FILE)))
    }

    pub fn load(&self) -> Result<DesktopPreferences, String> {
        let _guard = PREFERENCES_LOCK
            .lock()
            .map_err(|_| "Desktop preferences lock is poisoned".to_string())?;
        self.load_unlocked()
    }

    pub fn set_rua_root(&self, rua_root: &Path) -> Result<(), String> {
        self.update(|preferences| {
            preferences.rua_root = Some(rua_root.to_path_buf());
        })
    }

    pub fn set_locale(&self, locale: DesktopLocale) -> Result<(), String> {
        self.update(|preferences| {
            preferences.locale = locale;
        })
    }

    pub fn set_confirm_before_quit(&self, enabled: bool) -> Result<(), String> {
        self.update(|preferences| preferences.confirm_before_quit = enabled)
    }

    fn update(&self, mutate: impl FnOnce(&mut DesktopPreferences)) -> Result<(), String> {
        let _guard = PREFERENCES_LOCK
            .lock()
            .map_err(|_| "Desktop preferences lock is poisoned".to_string())?;
        let mut preferences = self.load_unlocked().unwrap_or_default();
        mutate(&mut preferences);
        let bytes = serde_json::to_vec_pretty(&preferences)
            .map_err(|error| format!("failed to serialize Desktop preferences: {error}"))?;
        atomic_write(&self.path, &bytes)
    }

    fn load_unlocked(&self) -> Result<DesktopPreferences, String> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(DesktopPreferences::default());
            }
            Err(error) => {
                return Err(format!(
                    "failed to read Desktop preferences {}: {error}",
                    self.path.display()
                ));
            }
        };
        serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "failed to parse Desktop preferences {}: {error}",
                self.path.display()
            )
        })
    }
}

pub fn resolve_configured_rua_root(
    explicit: Option<PathBuf>,
    persisted: Option<PathBuf>,
    home: PathBuf,
) -> PathBuf {
    explicit.or(persisted).unwrap_or_else(|| home.join(".rua"))
}

pub fn configured_rua_root(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let home = app
        .path()
        .home_dir()
        .map_err(|error| format!("failed to resolve home directory: {error}"))?;
    let explicit = std::env::var_os("RUA_DESKTOP_ROOT")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let persisted = DesktopPreferencesStore::for_app(app)?
        .load()?
        .rua_root
        .filter(|value| !value.as_os_str().is_empty());
    Ok(resolve_configured_rua_root(explicit, persisted, home))
}

pub fn rua_root_is_environment_managed() -> Option<OsString> {
    std::env::var_os("RUA_DESKTOP_ROOT").filter(|value| !value.is_empty())
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopLabels {
    pub about: &'static str,
    pub services: &'static str,
    pub hide: &'static str,
    pub hide_others: &'static str,
    pub show_all: &'static str,
    pub quit: &'static str,
    pub quit_confirmation: &'static str,
    pub quit_message: &'static str,
    pub confirm_before_quit: &'static str,
    pub cancel: &'static str,
    pub file: &'static str,
    pub edit: &'static str,
    pub view: &'static str,
    pub window: &'static str,
    pub help: &'static str,
    pub show: &'static str,
    pub switch_rua_root: &'static str,
    pub close_window: &'static str,
    pub undo: &'static str,
    pub redo: &'static str,
    pub cut: &'static str,
    pub copy: &'static str,
    pub paste: &'static str,
    pub select_all: &'static str,
    pub fullscreen: &'static str,
    pub minimize: &'static str,
    pub maximize: &'static str,
    pub choose_rua_root_title: &'static str,
    pub starting_title: &'static str,
    pub connecting: &'static str,
    pub connecting_elapsed: &'static str,
    pub ready_title: &'static str,
    pub opening_console: &'static str,
    pub timeout_title: &'static str,
    pub failed_title: &'static str,
    pub failed_message: &'static str,
    pub retry: &'static str,
    pub choose_rua_root: &'static str,
    pub runtime_timeout: &'static str,
}

impl DesktopLabels {
    pub const fn for_locale(locale: DesktopLocale) -> Self {
        match locale {
            DesktopLocale::English => Self {
                about: "About Rua",
                services: "Services",
                hide: "Hide Rua",
                hide_others: "Hide Others",
                show_all: "Show All",
                quit: "Quit Rua",
                quit_confirmation: "Quit Rua?",
                quit_message: "Quitting will stop local tasks running in Rua.",
                confirm_before_quit: "Confirm before quitting",
                cancel: "Cancel",
                file: "File",
                edit: "Edit",
                view: "View",
                window: "Window",
                help: "Help",
                show: "Show Rua",
                switch_rua_root: "Switch Rua Root and Restart Runtime...",
                close_window: "Close Window",
                undo: "Undo",
                redo: "Redo",
                cut: "Cut",
                copy: "Copy",
                paste: "Paste",
                select_all: "Select All",
                fullscreen: "Enter Full Screen",
                minimize: "Minimize",
                maximize: "Zoom",
                choose_rua_root_title: "Choose Rua Root",
                starting_title: "Starting Rua",
                connecting: "Connecting to the local Runtime...",
                connecting_elapsed: "Connecting to the local Runtime, waiting {seconds}s...",
                ready_title: "Rua is ready",
                opening_console: "Opening Console...",
                timeout_title: "Startup timed out",
                failed_title: "Startup failed",
                failed_message: "The local Runtime could not start.",
                retry: "Retry",
                choose_rua_root: "Choose Rua Root",
                runtime_timeout: "Rua Runtime did not become ready within {seconds} seconds.",
            },
            DesktopLocale::SimplifiedChinese => Self {
                about: "关于 Rua",
                services: "服务",
                hide: "隐藏 Rua",
                hide_others: "隐藏其他",
                show_all: "全部显示",
                quit: "退出 Rua",
                quit_confirmation: "确定退出 Rua？",
                quit_message: "退出后，Rua 中正在运行的本地任务将停止。",
                confirm_before_quit: "退出前提示",
                cancel: "取消",
                file: "文件",
                edit: "编辑",
                view: "显示",
                window: "窗口",
                help: "帮助",
                show: "显示 Rua",
                switch_rua_root: "切换 Rua 根目录并重启 Runtime...",
                close_window: "关闭窗口",
                undo: "撤销",
                redo: "重做",
                cut: "剪切",
                copy: "复制",
                paste: "粘贴",
                select_all: "全选",
                fullscreen: "进入全屏幕",
                minimize: "最小化",
                maximize: "缩放",
                choose_rua_root_title: "选择 Rua 根目录",
                starting_title: "正在启动 Rua",
                connecting: "正在连接本地运行时...",
                connecting_elapsed: "正在连接本地运行时，已等待 {seconds} 秒...",
                ready_title: "Rua 已就绪",
                opening_console: "正在打开控制台...",
                timeout_title: "启动超时",
                failed_title: "启动失败",
                failed_message: "本地运行时未能启动。",
                retry: "重试",
                choose_rua_root: "选择 Rua 根目录",
                runtime_timeout: "Rua Runtime 未能在 {seconds} 秒内就绪。",
            },
            DesktopLocale::Japanese => Self {
                about: "Rua について",
                services: "サービス",
                hide: "Rua を隠す",
                hide_others: "ほかを隠す",
                show_all: "すべてを表示",
                quit: "Rua を終了",
                quit_confirmation: "Rua を終了しますか？",
                quit_message: "終了すると、Rua で実行中のローカルタスクが停止します。",
                confirm_before_quit: "終了前に確認する",
                cancel: "キャンセル",
                file: "ファイル",
                edit: "編集",
                view: "表示",
                window: "ウインドウ",
                help: "ヘルプ",
                show: "Rua を表示",
                switch_rua_root: "Rua ルートを切り替えて Runtime を再起動...",
                close_window: "ウインドウを閉じる",
                undo: "取り消す",
                redo: "やり直す",
                cut: "カット",
                copy: "コピー",
                paste: "ペースト",
                select_all: "すべてを選択",
                fullscreen: "フルスクリーンにする",
                minimize: "しまう",
                maximize: "拡大／縮小",
                choose_rua_root_title: "Rua ルートを選択",
                starting_title: "Rua を起動中",
                connecting: "ローカル Runtime に接続しています...",
                connecting_elapsed:
                    "ローカル Runtime に接続しています。{seconds} 秒待機しています...",
                ready_title: "Rua の準備ができました",
                opening_console: "コンソールを開いています...",
                timeout_title: "起動がタイムアウトしました",
                failed_title: "起動に失敗しました",
                failed_message: "ローカル Runtime を起動できませんでした。",
                retry: "再試行",
                choose_rua_root: "Rua ルートを選択",
                runtime_timeout: "Rua Runtime は {seconds} 秒以内に準備できませんでした。",
            },
            DesktopLocale::Russian => Self {
                about: "О Rua",
                services: "Службы",
                hide: "Скрыть Rua",
                hide_others: "Скрыть остальные",
                show_all: "Показать все",
                quit: "Выйти из Rua",
                quit_confirmation: "Выйти из Rua?",
                quit_message: "При выходе локальные задачи Rua будут остановлены.",
                confirm_before_quit: "Подтверждать выход",
                cancel: "Отмена",
                file: "Файл",
                edit: "Правка",
                view: "Вид",
                window: "Окно",
                help: "Справка",
                show: "Показать Rua",
                switch_rua_root: "Сменить корневой каталог Rua и перезапустить Runtime...",
                close_window: "Закрыть окно",
                undo: "Отменить",
                redo: "Повторить",
                cut: "Вырезать",
                copy: "Копировать",
                paste: "Вставить",
                select_all: "Выбрать все",
                fullscreen: "Полноэкранный режим",
                minimize: "Свернуть",
                maximize: "Масштабировать",
                choose_rua_root_title: "Выберите корневой каталог Rua",
                starting_title: "Запуск Rua",
                connecting: "Подключение к локальному Runtime...",
                connecting_elapsed: "Подключение к локальному Runtime, ожидание {seconds} сек...",
                ready_title: "Rua готов",
                opening_console: "Открытие консоли...",
                timeout_title: "Время запуска истекло",
                failed_title: "Ошибка запуска",
                failed_message: "Не удалось запустить локальный Runtime.",
                retry: "Повторить",
                choose_rua_root: "Выбрать корневой каталог Rua",
                runtime_timeout: "Runtime Rua не запустился за {seconds} сек.",
            },
        }
    }
}

fn default_confirm_before_quit() -> bool {
    true
}

fn preferences_version() -> u8 {
    PREFERENCES_VERSION
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Desktop preferences path has no parent directory".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create Desktop settings directory: {error}"))?;
    let temp = parent.join(format!(
        ".{}.tmp.{}.{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        replace_file(&temp, path)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(format!(
            "failed to persist Desktop preferences {}: {error}",
            path.display()
        ));
    }
    Ok(())
}

fn replace_file(temp: &Path, target: &Path) -> std::io::Result<()> {
    match fs::rename(temp, target) {
        Ok(()) => Ok(()),
        #[cfg(windows)]
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            fs::remove_file(target)?;
            fs::rename(temp, target)
        }
        Err(error) => Err(error),
    }
}
