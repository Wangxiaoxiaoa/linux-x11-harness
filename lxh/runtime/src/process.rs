use std::process::Stdio;
use tokio::process::{Child, Command};

use lxh_core::LxhError;

pub struct ManagedProcess {
    child: Child,
    pid: u32,
}

impl ManagedProcess {
    pub async fn spawn(cmd: &str, args: &[&str], envs: &[(&str, &str)]) -> Result<Self, LxhError> {
        let mut command = Command::new(cmd);
        command
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        for (k, v) in envs {
            command.env(k, v);
        }

        // Harness displays have no input-method server, so IM variables
        // inherited from the user session only make XIM/GTK clients stall
        // (xterm can block ~5s in XIM init on XMODIFIERS=@im=<missing>).
        for var in ["XMODIFIERS", "GTK_IM_MODULE", "QT_IM_MODULE"] {
            command.env_remove(var);
        }

        let child = command
            .spawn()
            .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?;

        let pid = child.id().unwrap_or(0);
        Ok(Self { child, pid })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub async fn kill(&mut self) -> Result<(), LxhError> {
        self.child
            .kill()
            .await
            .map_err(|e| LxhError::ProcessKillFailed(e.to_string()))?;
        Ok(())
    }
}
