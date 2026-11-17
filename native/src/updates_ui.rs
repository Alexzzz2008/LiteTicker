use super::*;

impl App {
    pub(super) fn sync_update_view(&self) {
        if let Some(w) = self.settings_view.borrow().as_ref() {
            let pending = self.update_token.borrow().is_some();
            let available = self.available.borrow();
            w.set_update_busy(pending);
            w.set_download_enabled(!pending && available.is_some());
            w.set_latest_version(self.update_latest.borrow().as_str().into());
            let message = self.update_message.borrow();
            if !message.is_empty() {
                w.set_message(message.as_str().into());
            }
        }
    }
    pub(super) fn set_update_message(&self, value: String) {
        self.update_message.replace(value);
        self.sync_update_view();
    }
    fn start_update_request(&self) -> (u64, CancellationToken) {
        let token = CancellationToken::new();
        if let Some(previous) = self.update_token.replace(Some(token.clone())) {
            previous.cancel();
        }
        let id = self.update_gen.get().wrapping_add(1);
        self.update_gen.set(id);
        (id, token)
    }
    pub(super) fn check_update(&self) {
        if self.replay.is_some() {
            self.set_update_message("离线回放已暂停联网检查。".into());
            return;
        }
        let (id, token) = self.start_update_request();
        self.available.take();
        self.update_latest.replace("正在检查…".into());
        self.set_update_message("正在检查更新…".into());
        self.runtime.spawn(async move {
            let checked = async { update::UpdateClient::new()?.check(token).await }.await;
            deliver(Completion::Update(id, checked.map_err(|e| e.to_string())));
        });
    }
    pub(super) fn checked_update(&self, id: u64, result: Result<Option<Release>, String>) {
        if id != self.update_gen.get() {
            return;
        }
        self.update_token.take();
        let text = match result {
            Ok(Some(release)) => {
                let message = format!(
                    "发现新版本 v{}，点击「下载并重启」即可更新。",
                    release.version
                );
                #[cfg(windows)]
                if !self.modal() {
                    if let Some(platform) = self.platform.borrow().as_ref() {
                        platform.notify_update(&release.version);
                    }
                }
                self.update_latest.replace(format!("v{}", release.version));
                self.available.replace(Some(release));
                message
            }
            Ok(None) => {
                self.update_latest.replace(format!("v{VERSION}"));
                format!("已检查，当前使用最新正式版 v{VERSION}。")
            }
            Err(e) => {
                self.update_latest.replace("暂不可用".into());
                e
            }
        };
        self.set_update_message(text);
    }
    pub(super) fn download_update(&self) {
        if self.replay.is_some() || self.update_token.borrow().is_some() {
            return;
        }
        let Some(release) = self.available.borrow().clone() else {
            return;
        };
        let (id, token) = self.start_update_request();
        self.set_update_message("正在下载更新…".into());
        self.runtime.spawn(async move {
            let progress: update::Progress =
                Arc::new(move |pct| deliver(Completion::Progress(id, pct)));
            let file = async {
                update::UpdateClient::new()?
                    .download(&release, Some(progress), token)
                    .await
            }
            .await;
            deliver(Completion::Download(id, file.map_err(|e| format!("{e:#}"))));
        });
    }
    pub(super) fn downloaded_update(&self, id: u64, file: Result<DownloadedUpdate, String>) {
        if id != self.update_gen.get() {
            return;
        }
        self.update_token.take();
        let result = (|| -> Result<()> {
            let file = file.map_err(anyhow::Error::msg)?;
            let mut settings = self.settings.borrow().clone();
            let position = self.window.window().position();
            (settings.x, settings.y) = (position.x, position.y);
            self.store.save(&settings)?;
            update::launch_update(file)?;
            slint::quit_event_loop()?;
            Ok(())
        })();
        if let Err(e) = result {
            self.set_update_message(format!("更新未完成，{e:#}"));
        }
    }
}
