use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};

use lxh_core::LxhError;

const TERMINATE_GRACE: Duration = Duration::from_secs(3);

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

    /// Terminate the process gracefully: SIGTERM first so apps release
    /// their MIT-SHM segments and Xvfb cleans up its socket and lock file,
    /// then SIGKILL after the grace period if it ignores the signal.
    pub async fn kill(&mut self) -> Result<(), LxhError> {
        // tokio's Child::kill sends SIGKILL; deliver SIGTERM via the
        // `kill` binary instead of pulling in libc/nix for one signal.
        if self.pid > 0 {
            let _ = tokio::process::Command::new("kill")
                .args(["-TERM", &self.pid.to_string()])
                .status()
                .await;
            let deadline = tokio::time::Instant::now() + TERMINATE_GRACE;
            while tokio::time::Instant::now() < deadline {
                // `kill -0` succeeds while the process is alive.
                let alive = tokio::process::Command::new("kill")
                    .args(["-0", &self.pid.to_string()])
                    .status()
                    .await
                    .map(|s| s.success())
                    .unwrap_or(false);
                if !alive {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        self.child
            .kill()
            .await
            .map_err(|e| LxhError::ProcessKillFailed(e.to_string()))?;
        Ok(())
    }
}
