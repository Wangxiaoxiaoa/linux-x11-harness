use atspi::connection::AccessibilityConnection;
use atspi::connection::P2P;
use atspi::proxy::accessible::AccessibleProxy;
use atspi::proxy::proxy_ext::ProxyExt;
use atspi::CoordType;
use lxh_core::{A11yElement, AccessibilityTree, Bounds, LxhError};

#[derive(Clone)]
pub struct Element {
    pub index: usize,
    pub role: String,
    pub name: Option<String>,
    pub frame: Option<Bounds>,
    pub actions: Vec<String>,
    pub parent_index: Option<usize>,
    pub depth: usize,
}

pub async fn walk_tree(pid: u32) -> Result<Vec<Element>, LxhError> {
    // Zero tolerance for stale targets: an exited process may still be
    // registered in AT-SPI for a while; fail with a clear error instead of
    // returning an empty or half-valid tree.
    if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
        return Err(LxhError::InvalidArgument(format!(
            "pid {pid} is not running (the application may have exited)"
        )));
    }

    let conn = AccessibilityConnection::new()
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?;

    let root = conn
        .root_accessible_on_registry()
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?;

    let dbus = atspi::zbus::fdo::DBusProxy::new(conn.connection())
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?;

    let children = root
        .get_children()
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?;

    let mut app = None;
    for child_ref in children {
        if pid_of(&dbus, &child_ref).await == Some(pid) {
            app = Some(
                conn.object_as_accessible(&child_ref)
                    .await
                    .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?,
            );
            break;
        }
    }

    let app = app.ok_or_else(|| LxhError::DisplayNotFound(format!("pid {}", pid)))?;
    Ok(walk(&conn, &app).await)
}

async fn pid_of(
    dbus: &atspi::zbus::fdo::DBusProxy<'_>,
    oref: &atspi::ObjectRefOwned,
) -> Option<u32> {
    let name = oref.name_as_str()?;
    let bus = atspi::zbus::names::BusName::try_from(name.to_owned()).ok()?;
    dbus.get_connection_unix_process_id(bus).await.ok()
}

async fn walk<'a>(
    conn: &'a AccessibilityConnection,
    root: &'a AccessibleProxy<'a>,
) -> Vec<Element> {
    let mut elements = Vec::new();
    let mut stack: Vec<(AccessibleProxy<'a>, Option<usize>, usize)> = vec![(root.clone(), None, 0)];

    while let Some((node, parent_index, depth)) = stack.pop() {
        let index = elements.len();
        let role = node
            .get_role()
            .await
            .map(|r| r.to_string())
            .unwrap_or_default();
        let name = node.name().await.ok();

        let mut frame = None;
        let mut actions = Vec::new();

        if let Ok(proxies) = node.proxies().await {
            if let Ok(component) = proxies.component().await {
                if let Ok((x, y, w, h)) = component.get_extents(CoordType::Screen).await {
                    frame = Some(Bounds {
                        x,
                        y,
                        w: w as u32,
                        h: h as u32,
                    });
                }
            }
            if let Ok(action) = proxies.action().await {
                if let Ok(n) = action.n_actions().await {
                    for i in 0..n {
                        if let Ok(name) = action.get_name(i).await {
                            if !name.trim().is_empty() {
                                actions.push(name);
                            }
                        }
                    }
                }
            }
        }

        elements.push(Element {
            index,
            role,
            name,
            frame,
            actions,
            parent_index,
            depth,
        });

        if let Ok(children) = node.get_children().await {
            for child_ref in children.into_iter().rev() {
                if let Ok(child) = conn.object_as_accessible(&child_ref).await {
                    stack.push((child, Some(index), depth + 1));
                }
            }
        }
    }

    elements
}

pub fn accessibility_tree(elements: &[Element]) -> AccessibilityTree {
    AccessibilityTree {
        elements: elements
            .iter()
            .map(|e| A11yElement {
                index: e.index,
                role: e.role.clone(),
                name: e.name.clone(),
                frame: e.frame.clone(),
                actions: e.actions.clone(),
                parent_index: e.parent_index,
                depth: e.depth,
            })
            .collect(),
    }
}

pub async fn set_value(pid: u32, index: usize, value: &str) -> Result<(), LxhError> {
    let conn = AccessibilityConnection::new()
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?;

    let root = conn
        .root_accessible_on_registry()
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?;

    let dbus = atspi::zbus::fdo::DBusProxy::new(conn.connection())
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?;

    let children = root
        .get_children()
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?;

    let mut app = None;
    for child_ref in children {
        if pid_of(&dbus, &child_ref).await == Some(pid) {
            app = Some(
                conn.object_as_accessible(&child_ref)
                    .await
                    .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?,
            );
            break;
        }
    }

    let app = app.ok_or_else(|| LxhError::DisplayNotFound(format!("pid {}", pid)))?;

    let node = walk_to_index(&conn, &app, index)
        .await?
        .ok_or(LxhError::NotSupported)?;

    let proxies = node.proxies().await.map_err(|_| LxhError::NotSupported)?;
    let editable = proxies
        .editable_text()
        .await
        .map_err(|_| LxhError::NotSupported)?;

    editable
        .set_text_contents(value)
        .await
        .map_err(|_| LxhError::NotSupported)?;

    Ok(())
}

/// Resolve an application menu path (e.g. ["File", "Open"]) through
/// AT-SPI and invoke the final item. Path segments are matched against
/// direct children after trimming whitespace; matching is case-sensitive.
/// The menu bar itself is located by role ("menu bar"); if the application
/// has no menu bar the error says so.
pub async fn invoke_menu(pid: u32, path: &[String]) -> Result<(), LxhError> {
    if path.is_empty() {
        return Err(LxhError::InvalidArgument(
            "menu path must not be empty".into(),
        ));
    }

    let elements = walk_tree(pid).await?;
    let bar = elements
        .iter()
        .find(|e| e.role == "menu bar")
        .ok_or_else(|| {
            LxhError::InvalidArgument(format!("pid {pid} has no application menu bar"))
        })?;

    let mut current = bar.index;
    for (depth, segment) in path.iter().enumerate() {
        let wanted = segment.trim();
        let child = elements
            .iter()
            .find(|e| e.parent_index == Some(current) && e.name.as_deref() == Some(wanted))
            .ok_or_else(|| {
                LxhError::InvalidArgument(format!(
                    "menu path not found at segment {depth} ({}): no entry named {:?} under index {current}",
                    path[..=depth].join(" > "),
                    wanted
                ))
            })?;
        current = child.index;
    }

    let target = &elements[current];
    let action_name = target
        .actions
        .iter()
        .find(|a| a.as_str() == "press")
        .or_else(|| target.actions.first())
        .ok_or_else(|| {
            LxhError::InvalidArgument(format!(
                "menu item {:?} exposes no AT-SPI action to invoke",
                target.name
            ))
        })?;

    let conn = AccessibilityConnection::new()
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?;
    let root = conn
        .root_accessible_on_registry()
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?;
    let dbus = atspi::zbus::fdo::DBusProxy::new(conn.connection())
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?;
    let children = root
        .get_children()
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?;
    let mut app = None;
    for child_ref in children {
        if pid_of(&dbus, &child_ref).await == Some(pid) {
            app = Some(
                conn.object_as_accessible(&child_ref)
                    .await
                    .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?,
            );
            break;
        }
    }
    let app = app.ok_or_else(|| LxhError::DisplayNotFound(format!("pid {}", pid)))?;

    let node = walk_to_index(&conn, &app, current)
        .await?
        .ok_or(LxhError::NotSupported)?;
    let proxies = node.proxies().await.map_err(|_| LxhError::NotSupported)?;
    let action = proxies.action().await.map_err(|_| LxhError::NotSupported)?;

    // Invoke the action chosen from the element snapshot; match it by name
    // against the live object so the index stays honest.
    let n = action
        .n_actions()
        .await
        .map_err(|_| LxhError::NotSupported)?;
    let mut index_of = None;
    for i in 0..n {
        if let Ok(name) = action.get_name(i).await {
            if name == *action_name {
                index_of = Some(i);
                break;
            }
        }
    }
    let index_of = index_of.ok_or(LxhError::NotSupported)?;
    action
        .do_action(index_of)
        .await
        .map_err(|_| LxhError::NotSupported)?;

    Ok(())
}

async fn walk_to_index<'a>(
    conn: &'a AccessibilityConnection,
    root: &'a AccessibleProxy<'a>,
    target_index: usize,
) -> Result<Option<AccessibleProxy<'a>>, LxhError> {
    let mut stack = vec![root.clone()];
    let mut count = 0;

    while let Some(node) = stack.pop() {
        if count == target_index {
            return Ok(Some(node));
        }
        count += 1;

        if let Ok(children) = node.get_children().await {
            for child_ref in children.into_iter().rev() {
                if let Ok(child) = conn.object_as_accessible(&child_ref).await {
                    stack.push(child);
                }
            }
        }
    }

    Ok(None)
}
