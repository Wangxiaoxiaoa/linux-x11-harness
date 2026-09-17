use lxh_core::{DisplayInfo, LxhError};

use crate::process::ManagedProcess;
use crate::wm::OpenboxWM;
use crate::xserver::XvfbBackend;

#[derive(Clone)]
pub enum DisplayKind {
    Harness,
    External,
}

#[derive(Clone)]
pub struct DisplayConfig {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 800,
            depth: 24,
        }
    }
}

pub struct Display {
    id: String,
    display: String,
    kind: DisplayKind,
    width: u32,
    height: u32,
    xserver: Option<ManagedProcess>,
    wm: Option<ManagedProcess>,
    apps: std::sync::Mutex<Vec<ManagedProcess>>,
}

impl Display {
    pub async fn create(
        id: String,
        display: String,
        config: DisplayConfig,
    ) -> Result<Self, LxhError> {
        let xserver =
            XvfbBackend::start(&display, config.width, config.height, config.depth).await?;

        let wm = OpenboxWM::start(&display).await?;

        Ok(Self {
            id,
            display: display.clone(),
            kind: DisplayKind::Harness,
            width: config.width,
            height: config.height,
            xserver: Some(xserver),
            wm: Some(wm),
            apps: std::sync::Mutex::new(Vec::new()),
        })
    }

    pub async fn attach(id: String, display: String) -> Result<Self, LxhError> {
        Ok(Self {
            id,
            display: display.clone(),
            kind: DisplayKind::External,
            width: 0,
            height: 0,
            xserver: None,
            wm: None,
            apps: std::sync::Mutex::new(Vec::new()),
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn display(&self) -> &str {
        &self.display
    }

    pub fn kind(&self) -> &DisplayKind {
        &self.kind
    }

    pub fn is_external(&self) -> bool {
        matches!(self.kind, DisplayKind::External)
    }

    pub async fn launch_app(&self, command: &str, args: &[&str]) -> Result<u32, LxhError> {
        let proc = ManagedProcess::spawn(command, args, &[("DISPLAY", &self.display)]).await?;
        let pid = proc.pid();
        self.apps.lock().unwrap().push(proc);
        Ok(pid)
    }

    pub async fn terminate_app(&self, pid: u32) -> Result<(), LxhError> {
        let mut proc = {
            let mut apps = self.apps.lock().unwrap();
            let pos = apps
                .iter()
                .position(|p| p.pid() == pid)
                .ok_or_else(|| LxhError::InvalidArgument(format!("pid {} not found", pid)))?;
            apps.remove(pos)
        };
        proc.kill().await
    }

    pub fn info(&self) -> DisplayInfo {
        DisplayInfo {
            display: self.display.clone(),
            width: self.width,
            height: self.height,
            app_count: self.apps.lock().unwrap().len(),
        }
    }

    pub async fn destroy(&mut self) -> Result<(), LxhError> {
        if self.is_external() {
            return Err(LxhError::InvalidArgument(
                "cannot destroy external display".into(),
            ));
        }
        let mut apps = {
            let mut apps = self.apps.lock().unwrap();
            std::mem::take(&mut *apps)
        };
        for app in apps.iter_mut() {
            let _ = app.kill().await;
        }
        if let Some(ref mut wm) = self.wm {
            let _ = wm.kill().await;
        }
        if let Some(ref mut xserver) = self.xserver {
            let _ = xserver.kill().await;
        }
        Ok(())
    }
}
