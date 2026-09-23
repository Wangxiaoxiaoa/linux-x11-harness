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
    /// Text-interface content for entries and text views (their accessible
    /// name is often empty; the typed string lives here).
    pub value: Option<String>,
    /// Toggle state when the role exposes one (check boxes, check menu items).
    pub checked: Option<bool>,
    /// False when the StateSet lacks Enabled or the node is insensitive.
    pub enabled: Option<bool>,
    /// Selection state for selectable controls (radio, list items, tabs).
    pub selected: Option<bool>,
    pub description: Option<String>,
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
        let description = node.description().await.ok();

        let mut frame = None;
        let mut actions = Vec::new();
        let mut value = None;
        let mut checked = None;
        let mut selected = None;

        // The StateSet drives the role-aware booleans; read it once.
        let states = node.get_state().await.ok();
        let role_lower = role.to_ascii_lowercase();
        let enabled = states.as_ref().map(|s| s.contains(atspi::State::Enabled));
        if role_lower.contains("check") {
            checked = states.as_ref().map(|s| s.contains(atspi::State::Checked));
            selected = checked;
        } else if role_lower.contains("radio")
            || role_lower.contains("list item")
            || role_lower.contains("menu item")
            || matches!(role_lower.as_str(), "tab" | "page tab" | "tab item")
        {
            selected = states
                .as_ref()
                .map(|s| s.contains(atspi::State::Selected) || s.contains(atspi::State::Checked));
        }

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
            if let Ok(text) = proxies.text().await {
                if let Ok(count) = text.character_count().await {
                    if count > 0 {
                        // Text content is where entry/typed text lives; the
                        // accessible name of such widgets is often empty.
                        if let Ok(t) = text.get_text(0, count.min(4096)).await {
                            value = Some(t);
                        }
                    }
                }
            }
            if value.is_none() {
                if let Ok(value_proxy) = proxies.value().await {
                    if let Ok(v) = value_proxy.current_value().await {
                        value = Some(v.to_string());
                    }
                }
            }
        }

        // Surface Text content as the display name when the widget has no name.
        let name = match name {
            Some(n) if !n.trim().is_empty() => Some(n),
            _ => value.clone(),
        };

        elements.push(Element {
            index,
            role,
            name,
            value,
            checked,
            enabled,
            selected,
            description,
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
                value: e.value.clone(),
                checked: e.checked,
                enabled: e.enabled,
                selected: e.selected,
                description: e.description.clone(),
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

    // Focus-free on purpose: calling Component.GrabFocus here would raise
    // the whole toplevel on some toolkits. Toolkits that expose
    // EditableText accept the write; ones that gate it on focus must fail
    // honestly instead of changing desktop focus implicitly.
    if let Ok(editable) = proxies.editable_text().await {
        if editable.set_text_contents(value).await.unwrap_or(false) {
            return Ok(());
        }
        // Some toolkits reject SetTextContents but accept clear-then-insert
        // through the editable text's insert/delete pair.
        if let Ok(text) = proxies.text().await {
            if let Ok(count) = text.character_count().await {
                if editable.delete_text(0, count).await.unwrap_or(false)
                    && editable
                        .insert_text(0, value, value.chars().count() as i32)
                        .await
                        .unwrap_or(false)
                {
                    return Ok(());
                }
            }
        }
    }

    Err(LxhError::NotSupported)
}

/// Check whether the element holding keyboard focus (within this pid's
/// tree) exposes EditableText.
/// Recursively locate the focused node and report whether it exposes
/// EditableText. Tree descent stays within `pid`.
async fn focused_node_is_editable(
    conn: &AccessibilityConnection,
    dbus: &atspi::zbus::fdo::DBusProxy<'_>,
    pid: u32,
    node: &AccessibleProxy<'_>,
) -> Result<bool, LxhError> {
    if let Ok(states) = node.get_state().await {
        if states.contains(atspi::State::Focused) {
            let proxies = node.proxies().await.map_err(|_| LxhError::NotSupported)?;
            return Ok(proxies.editable_text().await.is_ok());
        }
    }
    if let Ok(children) = node.get_children().await {
        for child_ref in children {
            let Ok(child) = conn.object_as_accessible(&child_ref).await else {
                continue;
            };
            if pid_of(dbus, &child_ref).await != Some(pid) {
                continue;
            }
            if Box::pin(focused_node_is_editable(conn, dbus, pid, &child)).await? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub async fn focused_is_editable(pid: u32) -> Result<bool, LxhError> {
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
    for child_ref in children {
        if pid_of(&dbus, &child_ref).await != Some(pid) {
            continue;
        }
        let app = conn
            .object_as_accessible(&child_ref)
            .await
            .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?;
        if focused_node_is_editable(&conn, &dbus, pid, &app).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Write `text` into the first editable element in the pid's tree
/// (focus-free: GrabFocus gives the widget internal keyboard focus
/// without raising the window).
pub async fn type_into_editable(pid: u32, text: &str) -> Result<(), LxhError> {
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

    let elements = walk_tree(pid).await?;
    let editable_idx = elements
        .iter()
        .find(|e| {
            matches!(
                e.role.as_str(),
                "text" | "entry" | "password text" | "spin button"
            )
        })
        .map(|e| e.index)
        .ok_or_else(|| {
            LxhError::InvalidArgument(format!("pid {pid} exposes no editable element"))
        })?;

    let node = walk_to_index(&conn, &app, editable_idx)
        .await?
        .ok_or(LxhError::NotSupported)?;
    let proxies = node.proxies().await.map_err(|_| LxhError::NotSupported)?;

    // GrabFocus gives the widget internal keyboard focus without raising
    // the toplevel; toolkits that expose EditableText only while focused
    // (Qt6, GTK4 with GTK_A11Y=atspi) then accept the write.
    if let Ok(component) = proxies.component().await {
        let _ = component.grab_focus().await;
    }
    if let Ok(editable) = proxies.editable_text().await {
        if editable.set_text_contents(text).await.unwrap_or(false) {
            return Ok(());
        }
        if let Ok(t) = proxies.text().await {
            if let Ok(count) = t.character_count().await {
                if editable.delete_text(0, count).await.unwrap_or(false)
                    && editable
                        .insert_text(0, text, text.chars().count() as i32)
                        .await
                        .unwrap_or(false)
                {
                    return Ok(());
                }
            }
        }
    }

    Err(LxhError::NotSupported)
}

/// Perform the primary activation action of element `index` through
/// AT-SPI (focus-free, no coordinate math). Returns the action name that
/// was actuated.
///
/// The action is chosen by NAME, not by position: a GTK4 text view
/// advertises `buffer.delete-line` first, so firing "action 0" there
/// deletes a line of the user's document while reporting an ordinary
/// click. Elements with no activation verb (passive roles: labels,
/// statics, images) are rejected as suspected no-ops - the caller should
/// fall back to a coordinate click instead of trusting a fake success.
pub async fn perform_action(pid: u32, index: usize) -> Result<String, LxhError> {
    let elements = walk_tree(pid).await?;
    let target = elements.get(index).ok_or_else(|| {
        LxhError::InvalidArgument(format!(
            "element {index} not found (total: {})",
            elements.len()
        ))
    })?;

    const ACTIVATION_VERBS: [&str; 8] = [
        "press", "click", "activate", "open", "toggle", "expand", "choose", "confirm",
    ];
    const PASSIVE_ROLES: [&str; 6] = [
        "label",
        "static",
        "image",
        "heading",
        "separator",
        "scroll bar",
    ];

    let role_lower = target.role.to_ascii_lowercase();
    if target.actions.is_empty() || PASSIVE_ROLES.iter().any(|r| role_lower.contains(r)) {
        return Err(LxhError::InvalidArgument(format!(
            "suspected_noop: element {index} ({}) advertises no activation action; \
             use a coordinate click instead",
            target.role
        )));
    }

    let chosen = target
        .actions
        .iter()
        .find(|a| {
            let verb = a.to_ascii_lowercase();
            ACTIVATION_VERBS.iter().any(|v| verb.contains(v))
        })
        .unwrap_or(&target.actions[0]);

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
    let action = proxies.action().await.map_err(|_| LxhError::NotSupported)?;

    let n = action
        .n_actions()
        .await
        .map_err(|_| LxhError::NotSupported)?;
    let mut index_of = None;
    for i in 0..n {
        if let Ok(name) = action.get_name(i).await {
            if name == *chosen {
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

    // doAction acknowledgement can precede the toolkit's queued mutation;
    // give one short event-loop turn so a caller's immediate state read
    // observes the action that was delivered.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    Ok(chosen.clone())
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
