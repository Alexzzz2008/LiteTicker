use crate::model::{AppSettings, is_valid_shortcut, normalize_symbol};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Debug)]
pub struct SettingsStore {
    pub directory_path: PathBuf,
}
impl SettingsStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory_path: directory.into(),
        }
    }
    pub fn file_path(&self) -> PathBuf {
        self.directory_path.join("settings.json")
    }
    /// The first preview starts with the current user's v1 settings. Only the preview store is writable.
    pub fn load_preview(&self, installed: &SettingsStore) -> Result<AppSettings> {
        if self.file_path().try_exists()? || !installed.file_path().try_exists()? {
            return self.load();
        }
        let settings = installed.load()?;
        self.save(&settings)?;
        Ok(settings)
    }
    pub fn load(&self) -> Result<AppSettings> {
        let path = self.file_path();
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(AppSettings::default()),
            Err(e) => return Err(e).context("无法读取配置"),
        };
        let mut bytes = Vec::new();
        file.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 2 * 1024 * 1024 {
            bail!("配置文件超过大小上限");
        }
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
        let mut data: Value =
            serde_json::from_slice(bytes).context("配置文件损坏，未覆盖原文件")?;
        let values = data.as_object_mut().context("配置文件必须为对象")?;
        let defaults = AppSettings::default();
        for (name, min, max, fallback) in [
            ("TradeRefreshSeconds", 2, 3, 3),
            ("GlobalFontPercent", 80, 120, 100),
            ("QuoteFontPercent", 80, 150, 0),
            ("DetailsFontPercent", 80, 120, 0),
        ] {
            let n = values
                .get(name)
                .and_then(Value::as_i64)
                .filter(|n| (*n >= min) && (*n <= max))
                .unwrap_or(fallback);
            values.insert(name.into(), Value::from(n));
        }
        let automatic = values
            .get("AutoCheckUpdates")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        values.insert("AutoCheckUpdates".into(), Value::from(automatic));
        for (name, fallback) in [
            ("ToggleShortcut", defaults.toggle_shortcut),
            ("RefreshShortcut", defaults.refresh_shortcut),
        ] {
            let n = values
                .get(name)
                .and_then(Value::as_i64)
                .filter(|n| *n >= 0 && *n <= i32::MAX as i64)
                .map(|n| n as u32)
                .unwrap_or(fallback);
            values.insert(name.into(), Value::from(n));
        }
        if let Some(source) = values.get("Source") {
            if source.as_i64().is_some_and(|n| n != 0 && n != 1) {
                values.insert("Source".into(), Value::from(0));
            }
        }
        if values.get("Symbols").is_some_and(Value::is_null) {
            values.insert("Symbols".into(), Value::Array(Vec::new()));
        }
        let mut settings: AppSettings = serde_json::from_value(data).context("配置字段类型无效")?;
        if !matches!(settings.refresh_seconds, 1 | 2 | 3 | 5 | 10) {
            settings.refresh_seconds = 1;
        }
        settings.opacity_percent = settings.opacity_percent.clamp(60, 100);
        if !is_valid_shortcut(settings.toggle_shortcut) {
            settings.toggle_shortcut = defaults.toggle_shortcut;
        }
        if !is_valid_shortcut(settings.refresh_shortcut) {
            settings.refresh_shortcut = defaults.refresh_shortcut;
        }
        if settings.toggle_shortcut != 0 && settings.toggle_shortcut == settings.refresh_shortcut {
            settings.refresh_shortcut = 0;
        }
        let mut seen = HashSet::new();
        let mut symbols = Vec::new();
        for raw in &settings.symbols {
            let symbol = normalize_symbol(raw)?;
            if seen.insert(symbol.clone()) {
                symbols.push(symbol);
            }
        }
        settings.symbols = symbols;
        Ok(settings)
    }
    pub fn save(&self, settings: &AppSettings) -> Result<()> {
        fs::create_dir_all(&self.directory_path)?;
        let target = self.file_path();
        let (temporary, mut file) = create_temporary(&self.directory_path)?;
        let result = (|| -> Result<()> {
            file.write_all(&serde_json::to_vec(settings)?)?;
            file.sync_all()?;
            drop(file);
            if target.exists() {
                replace_with_backup(&temporary, &target)?;
            } else {
                fs::rename(&temporary, &target)?;
            }
            Ok(())
        })();
        if temporary.exists() {
            let _ = fs::remove_file(&temporary);
        }
        result.context("保存配置失败，原配置保留")
    }
}
fn create_temporary(directory: &Path) -> Result<(PathBuf, File)> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    for _ in 0..16 {
        let tick = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = directory.join(format!(
            "settings.json.{}.{tick}.{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    bail!("无法创建临时配置文件")
}
#[cfg(windows)]
fn replace_with_backup(temporary: &Path, target: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;
    let wide = |p: &Path| {
        p.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<u16>>()
    };
    let backup = target.with_file_name("settings.json.bak");
    let (a, b, c) = (wide(target), wide(temporary), wide(&backup));
    if unsafe {
        ReplaceFileW(
            a.as_ptr(),
            b.as_ptr(),
            c.as_ptr(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}
#[cfg(not(windows))]
fn replace_with_backup(temporary: &Path, target: &Path) -> Result<()> {
    let backup = target.with_file_name("settings.json.bak");
    let (staged_backup, staged_file) = create_temporary(target.parent().context("配置目录无效")?)?;
    drop(staged_file);
    let result = (|| -> Result<()> {
        fs::copy(target, &staged_backup)?;
        File::open(&staged_backup)?.sync_all()?;
        fs::rename(&staged_backup, backup)?;
        fs::rename(temporary, target)?;
        Ok(())
    })();
    if staged_backup.exists() {
        let _ = fs::remove_file(staged_backup);
    }
    result
}
