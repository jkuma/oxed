use gpui::{Action, actions};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

// If the zed binary doesn't use anything in this crate, it will be optimized away
// and the actions won't initialize. So we just provide an empty initialization function
// to be called from main.
//
// These may provide relevant context:
// https://github.com/rust-lang/rust/issues/47384
// https://github.com/mmastrac/rust-ctor/issues/280
pub fn init() {}

/// Opens a URL in the system's default web browser.
#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct OpenBrowser {
    pub url: Arc<str>,
}

/// Opens a zed:// URL within the application.
#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct OpenZedUrl {
    pub url: Arc<str>,
}

/// Opens the keymap to either add a keybinding or change an existing one
#[derive(PartialEq, Clone, Default, Action, JsonSchema, Serialize, Deserialize)]
#[action(namespace = zed, no_json, no_register)]
pub struct ChangeKeybinding {
    pub action: String,
}

actions!(
    zed,
    [
        /// Opens the settings editor.
        #[action(deprecated_aliases = ["zed_actions::OpenSettingsEditor"])]
        OpenSettings,
        /// Opens the settings JSON file.
        #[action(deprecated_aliases = ["zed_actions::OpenSettings"])]
        OpenSettingsFile,
        /// Opens project-specific settings.
        #[action(deprecated_aliases = ["zed_actions::OpenProjectSettings"])]
        OpenProjectSettings,
        /// Opens the default keymap file.
        OpenDefaultKeymap,
        /// Opens the user keymap file.
        #[action(deprecated_aliases = ["zed_actions::OpenKeymap"])]
        OpenKeymapFile,
        /// Opens the keymap editor.
        #[action(deprecated_aliases = ["zed_actions::OpenKeymapEditor"])]
        OpenKeymap,
        /// Quits the application.
        Quit,
        /// Shows information about Zed.
        About,
        /// Opens the documentation website.
        OpenDocs,
        /// Views open source licenses.
        OpenLicenses,
        /// Opens the performance profiler.
        OpenPerformanceProfiler,
        /// Opens the onboarding view.
        OpenOnboarding,
    ]
);

#[derive(PartialEq, Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionCategoryFilter {
    Themes,
    IconThemes,
    Languages,
    Grammars,
    LanguageServers,
    ContextServers,
    Snippets,
    DebugAdapters,
}

/// Opens the extensions management interface.
#[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct Extensions {
    /// Filters the extensions page down to extensions that are in the specified category.
    #[serde(default)]
    pub category_filter: Option<ExtensionCategoryFilter>,
    /// Focuses just the extension with the specified ID.
    #[serde(default)]
    pub id: Option<String>,
}

/// Decreases the font size in the editor buffer.
#[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct DecreaseBufferFontSize {
    #[serde(default)]
    pub persist: bool,
}

/// Increases the font size in the editor buffer.
#[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct IncreaseBufferFontSize {
    #[serde(default)]
    pub persist: bool,
}

/// Opens the settings editor at a specific path.
#[derive(PartialEq, Clone, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct OpenSettingsAt {
    /// A path to a specific setting (e.g. `theme.mode`)
    pub path: String,
    /// The settings file to select before opening `path`. When omitted, the
    /// existing settings file selection is preserved.
    #[serde(default)]
    pub target: Option<OpenSettingsAtTarget>,
}

#[derive(PartialEq, Clone, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct OpenSettingsPage {
    /// A settings page title (e.g. `AI`).
    pub page: String,
    /// The settings file to select before opening `page`. When omitted, the
    /// existing settings file selection is preserved.
    #[serde(default)]
    pub target: Option<OpenSettingsAtTarget>,
}

/// `OpenSettingsAt` path of the agent skills page in the settings UI.
pub const AGENT_SKILLS_SETTINGS_PATH: &str = "agent.skills";

/// `OpenSettingsAt` path of the agent sandbox permissions page in the settings
/// UI.
pub const AGENT_SANDBOX_SETTINGS_PATH: &str = "agent.sandbox_permissions";

#[derive(PartialEq, Clone, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpenSettingsAtTarget {
    User,
    Project { worktree_id: usize },
}

/// Resets the buffer font size to the default value.
#[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct ResetBufferFontSize {
    #[serde(default)]
    pub persist: bool,
}

/// Decreases the font size of the user interface.
#[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct DecreaseUiFontSize {
    #[serde(default)]
    pub persist: bool,
}

/// Increases the font size of the user interface.
#[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct IncreaseUiFontSize {
    #[serde(default)]
    pub persist: bool,
}

/// Resets the UI font size to the default value.
#[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct ResetUiFontSize {
    #[serde(default)]
    pub persist: bool,
}

/// Resets all zoom levels (UI and buffer font sizes, including in the agent panel) to their default values.
#[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct ResetAllZoom {
    #[serde(default)]
    pub persist: bool,
}

pub mod editor {
    use gpui::actions;
    actions!(
        editor,
        [
            /// Moves cursor up.
            MoveUp,
            /// Moves cursor down.
            MoveDown,
            /// Reveals the current file in the system file manager.
            RevealInFileManager,
        ]
    );
}

pub mod dev {
    use gpui::actions;

    actions!(
        dev,
        [
            /// Toggles the developer inspector for debugging UI elements.
            ToggleInspector,
            /// Cycles the debug frame-time overlay between hidden, current
            /// frame-time, and detailed frame-time statistics.
            ToggleFpsOverlay,
            /// Resets the debug frame-time overlay's statistics, except for the
            /// total frame count.
            ResetFrameOverlayStats,
            /// Opens the key context view for debugging keybindings.
            OpenKeyContextView
        ]
    );
}

pub mod workspace {
    use gpui::actions;

    actions!(
        workspace,
        [
            #[action(deprecated_aliases = ["editor::CopyPath", "outline_panel::CopyPath", "project_panel::CopyPath"])]
            CopyPath,
            #[action(deprecated_aliases = ["editor::CopyRelativePath", "outline_panel::CopyRelativePath", "project_panel::CopyRelativePath"])]
            CopyRelativePath,
            /// Opens the selected file with the system's default application.
            #[action(deprecated_aliases = ["project_panel::OpenWithSystem"])]
            OpenWithSystem,
        ]
    );
}

/// Describes which ref to base a new git worktree on. The worktree is
/// always created in a detached HEAD state; users can opt into creating
/// a branch afterwards from the worktree itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum NewWorktreeBranchTarget {
    /// Create a detached worktree from the current HEAD.
    #[default]
    CurrentBranch,
    /// Create a detached worktree at the tip of an existing branch.
    ExistingBranch { name: String },
    /// Create a detached worktree at the tip of a remote-tracking branch.
    RemoteBranch {
        remote_name: String,
        branch_name: String,
    },
}

/// Creates a new git worktree and switches the workspace to it.
/// Dispatched by the unified worktree picker when the user selects a "Create new worktree" entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = git)]
#[serde(deny_unknown_fields)]
pub struct CreateWorktree {
    /// When this is None, Zed will randomly generate a worktree name.
    pub worktree_name: Option<String>,
    pub branch_target: NewWorktreeBranchTarget,
}

/// Switches the workspace to an existing linked worktree.
/// Dispatched by the unified worktree picker when the user selects an existing worktree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = git)]
#[serde(deny_unknown_fields)]
pub struct SwitchWorktree {
    pub path: PathBuf,
    pub display_name: String,
}

/// Opens an existing worktree in a new window.
/// Dispatched by the worktree picker's "Open in New Window" button.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = git)]
#[serde(deny_unknown_fields)]
pub struct OpenWorktreeInNewWindow {
    pub path: PathBuf,
}

pub mod git {
    use gpui::actions;

    actions!(
        git,
        [
            /// Checks out a different git branch.
            CheckoutBranch,
            /// Switches to a different git branch.
            Switch,
            /// Selects a different repository.
            SelectRepo,
            /// Filter remotes.
            FilterRemotes,
            /// Create a git remote.
            CreateRemote,
            /// Opens the git branch selector.
            #[action(deprecated_aliases = ["branches::OpenRecent"])]
            Branch,
            /// Shows uncommitted changes across the project.
            ViewUncommittedChanges,
            /// Shows unstaged changes across the project.
            ViewUnstagedChanges,
            /// Shows staged changes across the project.
            ViewStagedChanges,
            /// Opens the git stash selector.
            ViewStash,
            /// Opens the git worktree selector.
            Worktree,
            /// Creates a pull request for the current branch.
            CreatePullRequest
        ]
    );
}

pub mod toast {
    use gpui::actions;

    actions!(
        toast,
        [
            /// Runs the action associated with a toast notification.
            RunAction
        ]
    );
}

pub mod command_palette {
    use gpui::actions;

    actions!(
        command_palette,
        [
            /// Toggles the command palette.
            Toggle,
        ]
    );
}

pub mod text_finder {
    use gpui::actions;

    actions!(
        text_finder,
        [
            /// Opens the Project Search Picker.
            Toggle,
        ]
    );
}

pub mod project_panel {
    use gpui::actions;

    actions!(
        project_panel,
        [
            /// Toggles the project panel.
            Toggle,
            /// Toggles focus on the project panel.
            ToggleFocus
        ]
    );
}
pub mod theme {
    use gpui::actions;

    actions!(theme, [ToggleMode]);
}

pub mod theme_selector {
    use gpui::Action;
    use schemars::JsonSchema;
    use serde::Deserialize;

    /// Toggles the theme selector interface.
    #[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
    #[action(namespace = theme_selector)]
    #[serde(deny_unknown_fields)]
    pub struct Toggle {
        /// A list of theme names to filter the theme selector down to.
        pub themes_filter: Option<Vec<String>>,
    }
}

pub mod icon_theme_selector {
    use gpui::Action;
    use schemars::JsonSchema;
    use serde::Deserialize;

    /// Toggles the icon theme selector interface.
    #[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
    #[action(namespace = icon_theme_selector)]
    #[serde(deny_unknown_fields)]
    pub struct Toggle {
        /// A list of icon theme names to filter the theme selector down to.
        pub themes_filter: Option<Vec<String>>,
    }
}

pub mod search {
    use gpui::{Action, actions};

    /// Opens a new project search filtered down to the given directory.
    ///
    /// An internal forwarding action: the user-facing, keybindable entry
    /// point is `project_panel::NewSearchInDirectory`, which resolves the
    /// selected directory and dispatches this action with it.
    #[derive(Clone, Debug, Default, PartialEq, Action)]
    #[action(namespace = search, no_json, no_register)]
    pub struct NewSearchInDirectory {
        pub directory: String,
    }

    actions!(
        search,
        [
            /// Focuses on the search input field.
            FocusSearch,
            /// Selects the next search match.
            SelectNextMatch,
            /// Selects the previous search match.
            SelectPreviousMatch,
            /// Toggles case-sensitive search.
            ToggleCaseSensitive,
            /// Toggles searching in ignored files.
            ToggleIncludeIgnored
        ]
    );
}
pub mod buffer_search {
    use gpui::{Action, actions};
    use schemars::JsonSchema;
    use serde::Deserialize;

    /// Opens the buffer search interface with the specified configuration.
    #[derive(PartialEq, Clone, Deserialize, JsonSchema, Action)]
    #[action(namespace = buffer_search)]
    #[serde(deny_unknown_fields)]
    pub struct Deploy {
        #[serde(default = "util::serde::default_true")]
        pub focus: bool,
        #[serde(default)]
        pub replace_enabled: bool,
        #[serde(default)]
        pub selection_search_enabled: bool,
    }

    impl Deploy {
        pub fn find() -> Self {
            Self {
                focus: true,
                replace_enabled: false,
                selection_search_enabled: false,
            }
        }

        pub fn replace() -> Self {
            Self {
                focus: true,
                replace_enabled: true,
                selection_search_enabled: false,
            }
        }
    }

    actions!(
        buffer_search,
        [
            /// Deploys the search and replace interface.
            DeployReplace,
            /// Dismisses the search bar.
            Dismiss,
            /// Focuses back on the editor.
            FocusEditor,
            /// Sets the search query from the selection or word under cursor.
            UseSelectionForFind,
        ]
    );
}
pub mod settings_profile_selector {
    use gpui::Action;
    use schemars::JsonSchema;
    use serde::Deserialize;

    #[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
    #[action(namespace = settings_profile_selector)]
    pub struct Toggle;
}

/// Opens the recent projects interface.
#[derive(PartialEq, Clone, Deserialize, Default, JsonSchema, Action)]
#[action(namespace = projects)]
#[serde(deny_unknown_fields)]
pub struct OpenRecent {
    #[serde(default)]
    pub create_new_window: Option<bool>,
}

/// Creates a project from a selected template.
#[derive(PartialEq, Clone, Deserialize, Default, JsonSchema, Action)]
#[action(namespace = projects)]
#[serde(deny_unknown_fields)]
pub struct OpenRemote {
    #[serde(default)]
    pub from_existing_connection: bool,
    #[serde(default)]
    pub create_new_window: Option<bool>,
}

/// Opens the dev container connection modal.
#[derive(PartialEq, Clone, Deserialize, Default, JsonSchema, Action)]
#[action(namespace = projects)]
#[serde(deny_unknown_fields)]
pub struct OpenDevContainer;

pub mod outline {
    use std::sync::OnceLock;

    use gpui::{AnyView, App, Window, actions};

    actions!(
        outline,
        [
            #[action(name = "Toggle")]
            ToggleOutline
        ]
    );
    /// A pointer to outline::toggle function, exposed here to sewer the breadcrumbs <-> outline dependency.
    pub static TOGGLE_OUTLINE: OnceLock<fn(AnyView, &mut Window, &mut App)> = OnceLock::new();
}

actions!(
    git_onboarding,
    [
        /// Opens the git integration onboarding modal.
        OpenGitIntegrationOnboarding
    ]
);
actions!(
    call_hierarchy,
    [ShowIncomingCalls, ShowOutgoingCalls, ToggleDirection]
);

pub mod preview {
    pub mod markdown {
        use gpui::actions;

        actions!(
            markdown,
            [
                /// Opens a markdown preview for the current file.
                OpenPreview,
                /// Opens a markdown preview in a split pane.
                OpenPreviewToTheSide,
            ]
        );
    }

    pub mod svg {
        use gpui::actions;

        actions!(
            svg,
            [
                /// Opens an SVG preview for the current file.
                OpenPreview,
                /// Opens an SVG preview in a split pane.
                OpenPreviewToTheSide,
            ]
        );
    }
}

pub mod git_panel {
    use gpui::actions;

    actions!(
        git_panel,
        [
            /// Toggles focus on the git panel.
            ToggleFocus,
        ]
    );
}
