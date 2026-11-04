use anyhow::{Context as _, Result};
use gpui::{
    App, Context, Entity, EntityId, ManagedView, Render, Subscription, Task, TaskExt, Window,
    WindowId,
};
use project::Project;
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use ui::prelude::*;
use util::ResultExt;
use util::path_list::PathList;

use crate::{
    CloseIntent, CloseWindow, DockPosition, Event as WorkspaceEvent, Item, ModalView, OpenMode,
    Panel, Workspace, WorkspaceId, client_side_decorations,
    persistence::model::MultiWorkspaceState,
};

/// The root of a window: hosts the workspace currently displayed in it.
pub struct MultiWorkspace {
    window_id: WindowId,
    workspace: Entity<Workspace>,
    /// Shared with every workspace this window has displayed, so a workspace that
    /// is being replaced can tell it no longer owns the window title and edited
    /// indicator without reading this entity.
    active_workspace_id: Rc<Cell<EntityId>>,
    pending_removal_tasks: Vec<Task<()>>,
    _serialize_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl MultiWorkspace {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let release_subscription = cx.on_release(|this: &mut MultiWorkspace, _cx| {
            if let Some(task) = this._serialize_task.take() {
                task.detach();
            }
            for task in std::mem::take(&mut this.pending_removal_tasks) {
                task.detach();
            }
        });
        let active_workspace_id = Rc::new(Cell::new(workspace.entity_id()));
        let mut this = Self {
            window_id: window.window_handle().window_id(),
            workspace: workspace.clone(),
            active_workspace_id,
            pending_removal_tasks: Vec::new(),
            _serialize_task: None,
            _subscriptions: vec![release_subscription],
        };
        this.register_workspace(&workspace, window, cx);
        this
    }

    pub fn close_window(&mut self, _: &CloseWindow, window: &mut Window, cx: &mut Context<Self>) {
        let Some(window_handle) = window.window_handle().downcast::<Self>() else {
            log::error!("cannot close a window whose root is not a MultiWorkspace");
            return;
        };
        cx.spawn(async move |_, cx| {
            if !crate::prepare_window_to_close(window_handle, CloseIntent::CloseWindow, cx)
                .await
                .context("preparing the window to close")?
            {
                return anyhow::Ok(());
            }

            crate::flush_windows_serialization(&[window_handle], cx).await;

            window_handle
                .update(cx, |_, window, _cx| {
                    window.remove_window();
                })
                .context("removing the closed window")?;

            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn register_workspace(
        &mut self,
        workspace: &Entity<Workspace>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe_in(workspace, window, |this, workspace, event, window, cx| {
            if let WorkspaceEvent::Activate = event {
                this.activate(workspace.clone(), window, cx);
            }
        })
        .detach();
        let weak_self = cx.weak_entity();
        let active_workspace_id = self.active_workspace_id.clone();
        workspace.update(cx, |workspace, _cx| {
            workspace.set_multi_workspace(weak_self, active_workspace_id);
        });
    }

    fn workspace_for_paths(&self, path_list: &PathList, cx: &App) -> Option<Entity<Workspace>> {
        let root_paths = PathList::new(&self.workspace.read(cx).root_paths(cx));
        (root_paths == *path_list).then(|| self.workspace.clone())
    }

    /// Activates the displayed workspace if its paths match, or opens the paths
    /// in a new workspace (deserializing its saved state from the database).
    /// Never searches other windows or matches workspaces with a superset of
    /// the requested paths.
    pub fn find_or_create_workspace(
        &mut self,
        path_list: PathList,
        init: Option<Box<dyn FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + Send>>,
        open_mode: OpenMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Workspace>>> {
        if let Some(workspace) = self.workspace_for_paths(&path_list, cx) {
            self.activate(workspace.clone(), window, cx);
            return Task::ready(Ok(workspace));
        }

        let app_state = self.workspace.read(cx).app_state().clone();
        let requesting_window = window.window_handle().downcast::<MultiWorkspace>();
        cx.spawn(async move |_this, cx| {
            let result = cx
                .update(|cx| {
                    Workspace::new_local(
                        path_list.paths().to_vec(),
                        app_state,
                        requesting_window,
                        None,
                        init,
                        open_mode,
                        cx,
                    )
                })
                .await?;
            Ok(result.workspace)
        })
    }

    pub fn workspace(&self) -> &Entity<Workspace> {
        &self.workspace
    }

    /// Displays `workspace` in this window, replacing the current one.
    pub fn activate(
        &mut self,
        workspace: Entity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace == workspace {
            self.focus_active_workspace(window, cx);
            return;
        }

        self.register_workspace(&workspace, window, cx);
        let previous_workspace = std::mem::replace(&mut self.workspace, workspace.clone());

        // Publish the new active workspace before anyone reads the shared cell
        // to decide who owns the window chrome.
        self.active_workspace_id.set(workspace.entity_id());
        self.detach_workspace(&previous_workspace, cx);

        // The previous workspace left the title and edited indicator
        // reflecting its own state, so re-apply them from the new one.
        workspace.update(cx, |workspace, cx| {
            workspace.refresh_window_state(window, cx);
        });

        self.serialize(cx);
        self.focus_active_workspace(window, cx);
        cx.notify();
    }

    /// Clears the session state and database binding of a workspace that is no
    /// longer displayed. Its database row is kept so it still appears in the
    /// recent projects.
    fn detach_workspace(&mut self, workspace: &Entity<Workspace>, cx: &mut Context<Self>) {
        workspace.update(cx, |workspace, _cx| {
            workspace.session_id.take();
            workspace._schedule_serialize_workspace.take();
            workspace._serialize_workspace_task.take();
        });

        if let Some(workspace_id) = workspace.read(cx).database_id() {
            let db = crate::persistence::WorkspaceDb::global(cx);
            self.pending_removal_tasks.retain(|task| !task.is_ready());
            self.pending_removal_tasks
                .push(cx.background_spawn(async move {
                    db.set_session_binding(workspace_id, None, None)
                        .await
                        .log_err();
                }));
        }
    }

    /// Closes the displayed project and replaces it with an empty workspace.
    /// Resolves to `false` when the user cancels a save prompt.
    pub fn close_project(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<bool>> {
        let workspace = self.workspace.clone();
        cx.spawn_in(window, async move |this, cx| {
            let should_continue = workspace
                .update_in(cx, |workspace, window, cx| {
                    workspace.prepare_to_close(CloseIntent::ReplaceWindow, window, cx)
                })?
                .await?;
            if !should_continue {
                return Ok(false);
            }

            this.update_in(cx, |this, window, cx| {
                let app_state = this.workspace.read(cx).app_state().clone();
                let project = Project::local(
                    app_state.http_client.clone(),
                    app_state.node_runtime.clone(),
                    app_state.languages.clone(),
                    app_state.fs.clone(),
                    None,
                    project::LocalProjectFlags::default(),
                    cx,
                );
                let empty_workspace =
                    cx.new(|cx| Workspace::new(None, project, app_state, window, cx));
                this.activate(empty_workspace, window, cx);
            })?;
            Ok(true)
        })
    }

    pub fn open_project(
        &mut self,
        paths: Vec<PathBuf>,
        open_mode: OpenMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Workspace>>> {
        let workspace = self.workspace.clone();
        cx.spawn_in(window, async move |_this, cx| {
            let should_continue = workspace
                .update_in(cx, |workspace, window, cx| {
                    workspace.prepare_to_close(CloseIntent::ReplaceWindow, window, cx)
                })?
                .await?;
            if should_continue {
                workspace
                    .update_in(cx, |workspace, window, cx| {
                        workspace.open_workspace_for_paths(open_mode, paths, window, cx)
                    })?
                    .await
            } else {
                Ok(workspace)
            }
        })
    }

    pub fn serialize(&mut self, cx: &mut Context<Self>) {
        self._serialize_task = Some(cx.spawn(async move |this, cx| {
            let Ok(task) = this.update(cx, |this, cx| this.serialize_now(cx)) else {
                return;
            };
            task.await;
        }));
    }

    fn serialize_now(&mut self, cx: &mut Context<Self>) -> impl Future<Output = ()> + use<> {
        let state = MultiWorkspaceState {
            active_workspace_id: self.workspace.read(cx).database_id(),
        };
        let window_id = self.window_id;
        let kvp = db::kvp::KeyValueStore::global(cx);
        async move {
            crate::persistence::write_multi_workspace_state(&kvp, window_id, state).await;
        }
    }

    /// Used by the quit handler to ensure pending DB writes
    /// complete before the process exits.
    pub fn flush_serialization(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self._serialize_task.take();
        let serialization = self.serialize_now(cx);
        cx.spawn(async move |_, _| serialization.await)
    }

    pub fn flush_pending_serialization(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Task<()>> {
        let mut tasks = vec![self.workspace.update(cx, |workspace, cx| {
            workspace.flush_serialization(window, cx)
        })];
        tasks.append(&mut self.take_pending_removal_tasks());
        tasks.push(self.flush_serialization(cx));
        tasks
    }

    pub fn focus_active_workspace(&self, window: &mut Window, cx: &mut App) {
        let focus_handle = self.workspace().read(cx).fallback_focus_handle(window, cx);
        window.focus(&focus_handle, cx);
    }

    pub fn panel<T: Panel>(&self, cx: &App) -> Option<Entity<T>> {
        self.workspace().read(cx).panel::<T>(cx)
    }

    pub fn active_modal<V: ManagedView + 'static>(&self, cx: &App) -> Option<Entity<V>> {
        self.workspace().read(cx).active_modal::<V>(cx)
    }

    pub fn add_panel<T: Panel>(
        &mut self,
        panel: Entity<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace().update(cx, |workspace, cx| {
            workspace.add_panel(panel, window, cx);
        });
    }

    pub fn focus_panel<T: Panel>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<T>> {
        self.workspace()
            .update(cx, |workspace, cx| workspace.focus_panel::<T>(window, cx))
    }

    // used in a test
    pub fn toggle_modal<V: ModalView, B>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        build: B,
    ) where
        B: FnOnce(&mut Window, &mut gpui::Context<V>) -> V,
    {
        self.workspace().update(cx, |workspace, cx| {
            workspace.toggle_modal(window, cx, build);
        });
    }

    pub fn toggle_dock(
        &mut self,
        dock_side: DockPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace().update(cx, |workspace, cx| {
            workspace.toggle_dock(dock_side, window, cx);
        });
    }

    pub fn active_item_as<I: 'static>(&self, cx: &App) -> Option<Entity<I>> {
        self.workspace().read(cx).active_item_as::<I>(cx)
    }

    pub fn items_of_type<'a, T: Item>(
        &'a self,
        cx: &'a App,
    ) -> impl 'a + Iterator<Item = Entity<T>> {
        self.workspace().read(cx).items_of_type::<T>(cx)
    }

    pub fn database_id(&self, cx: &App) -> Option<WorkspaceId> {
        self.workspace().read(cx).database_id()
    }

    pub fn take_pending_removal_tasks(&mut self) -> Vec<Task<()>> {
        std::mem::take(&mut self.pending_removal_tasks)
            .into_iter()
            .filter(|task| !task.is_ready())
            .collect()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_random_database_id(&mut self, cx: &mut Context<Self>) {
        self.workspace.update(cx, |workspace, _cx| {
            workspace.set_random_database_id();
        });
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn test_new(project: Entity<Project>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let workspace = cx.new(|cx| Workspace::test_new(project, window, cx));
        Self::new(workspace, window, cx)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn test_activate_project(
        &mut self,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Workspace> {
        let workspace = cx.new(|cx| Workspace::test_new(project, window, cx));
        self.activate(workspace.clone(), window, cx);
        workspace
    }

    /// Assigns a database ID to the workspace if it has none, then flushes its
    /// state, the window state and its session binding so they can be restored.
    #[cfg(any(test, feature = "test-support"))]
    pub fn flush_all_serialization(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Task<()>> {
        self.workspace.update(cx, |workspace, _cx| {
            if workspace.database_id().is_none() {
                workspace.set_random_database_id();
            }
        });

        let session_id = self.workspace.read(cx).session_id();
        let window_id = window.window_handle().window_id().as_u64();

        let mut tasks: Vec<Task<()>> = vec![self.workspace.update(cx, |workspace, cx| {
            workspace.flush_serialization(window, cx)
        })];
        if let Some(workspace_id) = self.workspace.read(cx).database_id() {
            let db = crate::persistence::WorkspaceDb::global(cx);
            tasks.push(cx.background_spawn(async move {
                db.set_session_binding(workspace_id, session_id, Some(window_id))
                    .await
                    .log_err();
            }));
        }
        tasks.push(self.flush_serialization(cx));
        tasks
    }
}

impl Render for MultiWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui_font = theme_settings::setup_ui_font(window, cx);
        let text_color = cx.theme().colors().text;

        let workspace = self.workspace.clone();
        let workspace_key_context = workspace.update(cx, |workspace, cx| workspace.key_context(cx));
        let root = workspace.update(cx, |workspace, cx| workspace.actions(h_flex(), window, cx));

        client_side_decorations(
            root.key_context(workspace_key_context)
                .relative()
                .size_full()
                .font(ui_font)
                .text_color(text_color)
                .on_action(cx.listener(Self::close_window))
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .size_full()
                        .overflow_hidden()
                        .child(workspace.clone()),
                )
                .child(workspace.read(cx).modal_layer.clone()),
            window,
            cx,
        )
    }
}
