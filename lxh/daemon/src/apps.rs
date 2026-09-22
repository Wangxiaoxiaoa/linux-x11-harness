//! Application discovery: running processes merged with XDG desktop
//! entries, so agents can answer "is X installed?" and "is X running?"
//! and get a launchable path for `lxh_app_launch`.

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct AppEntry {
    pub name: String,
    pub running: bool,
    /// Process pid when running, `None` otherwise.
    pub pid: Option<u32>,
    /// The launcher command (first token of `Exec=`, field codes stripped).
    /// Present for apps with an XDG desktop entry; pass to `lxh_app_launch`.
    pub launch_path: Option<String>,
    /// XDG desktop file id: the `.desktop` path relative to its
    /// `applications/` root with separators replaced by `-`.
    pub bundle_id: Option<String>,
}

/// Running processes from `/proc` merged with installed apps from XDG
/// desktop entries:
///
/// - every live process becomes one entry (`running: true`), matched by
///   executable basename to a desktop entry when one exists;
/// - installed-but-not-running entries follow (`running: false`);
/// - desktop entries with `NoDisplay=true`, `Hidden=true`,
///   `Type!=Application` or missing `Name`/`Exec` are skipped;
/// - XDG_DATA_HOME entries override XDG_DATA_DIRS entries with the same
///   bundle id.
pub fn list_apps() -> Vec<AppEntry> {
    let installed = desktop_entries();
    let mut by_exe: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, (_, _, launch_path)) in installed.iter().enumerate() {
        let basename = basename_of(launch_path).to_ascii_lowercase();
        if !basename.is_empty() {
            by_exe.entry(basename).or_default().push(i);
        }
    }

    let mut out: Vec<AppEntry> = Vec::new();
    let mut consumed = std::collections::HashSet::new();
    for (pid, cmdline, comm) in running_processes() {
        let basename = basename_of(&cmdline).to_ascii_lowercase();
        let basename = if basename.is_empty() {
            comm.to_ascii_lowercase()
        } else {
            basename
        };
        let matched = by_exe
            .get(&basename)
            .and_then(|candidates| candidates.iter().find(|i| !consumed.contains(*i)).copied());
        if let Some(i) = matched {
            consumed.insert(i);
        }
        let (name, bundle_id, launch_path) = match matched {
            Some(i) => {
                let (bundle_id, name, launch_path) = &installed[i];
                (
                    name.clone(),
                    Some(bundle_id.clone()),
                    Some(launch_path.clone()),
                )
            }
            None => (comm.clone(), None, None),
        };
        out.push(AppEntry {
            name,
            running: true,
            pid: Some(pid),
            bundle_id,
            launch_path,
        });
    }

    for (i, (bundle_id, name, launch_path)) in installed.iter().enumerate() {
        if !consumed.contains(&i) {
            out.push(AppEntry {
                name: name.clone(),
                running: false,
                pid: None,
                bundle_id: Some(bundle_id.clone()),
                launch_path: Some(launch_path.clone()),
            });
        }
    }

    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

/// Live user processes: (pid, cmdline argv[0], comm). Kernel threads have
/// no cmdline and are skipped.
fn running_processes() -> Vec<(u32, String, String)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(cmdline) = std::fs::read_to_string(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        // cmdline is NUL-separated; kernel threads have none.
        let argv0 = cmdline.split('\0').next().unwrap_or_default().to_string();
        if argv0.is_empty() {
            continue;
        }
        let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) else {
            continue;
        };
        out.push((pid, argv0, comm.trim().to_string()));
    }
    out
}

/// Walk `$XDG_DATA_HOME/applications` then each `$XDG_DATA_DIRS` entry's
/// `applications/` subdir, yielding (bundle_id, name, launch_path).
fn desktop_entries() -> Vec<(String, String, String)> {
    let mut dirs = Vec::new();
    let data_home = std::env::var("XDG_DATA_HOME")
        .unwrap_or_else(|_| format!("{}/.local/share", std::env::var("HOME").unwrap_or_default()));
    dirs.push(format!("{data_home}/applications"));
    let data_dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for d in data_dirs.split(':') {
        if !d.is_empty() {
            dirs.push(format!("{d}/applications"));
        }
    }

    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let Some(bundle_id) = desktop_file_id(&dir, &path) else {
                continue;
            };
            if let Some((name, launch_path)) = parse_desktop_file(&path) {
                out.push((bundle_id, name, launch_path));
            }
        }
    }
    out
}

/// `.desktop` path relative to its `applications/` root, separators
/// replaced with `-` and the suffix stripped
/// (`kde4/konqbrowser.desktop` -> `kde4-konqbrowser`).
fn desktop_file_id(root: &str, path: &std::path::Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?.to_string_lossy();
    let id = rel.strip_suffix(".desktop")?;
    Some(id.replace('/', "-"))
}

/// Parse the `[Desktop Entry]` section. Returns (Name, launch command).
fn parse_desktop_file(path: &std::path::Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut name = None;
    let mut exec = None;
    let mut in_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(header) = trimmed.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
            if in_section {
                break; // next section started
            }
            in_section = header == "Desktop Entry";
            continue;
        }
        if !in_section || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            match key.trim() {
                "NoDisplay" | "Hidden" if value.trim() == "true" => return None,
                "Type" if !value.trim().is_empty() && value.trim() != "Application" => return None,
                "Name" if name.is_none() => name = Some(value.trim().to_string()),
                "Exec" if exec.is_none() => exec = Some(value.trim().to_string()),
                _ => {}
            }
        }
    }

    let name = name?;
    let exec = exec?;
    let launch_path = strip_exec_field_codes(&exec);
    if launch_path.is_empty() {
        return None;
    }
    Some((name, launch_path))
}

/// First token of `Exec=` with field codes (`%f`, `%F`, `%u`, `%U`, ...)
/// stripped and surrounding quotes removed.
fn strip_exec_field_codes(exec: &str) -> String {
    let first = exec.split_whitespace().next().unwrap_or_default();
    first.replace('%', "").trim_matches('"').to_string()
}

fn basename_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}
