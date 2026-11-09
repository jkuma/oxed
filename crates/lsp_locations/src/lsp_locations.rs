use std::any::TypeId;
use std::ops::Range;
use std::sync::Arc;

use collections::HashMap;
use editor::actions::{
    FindAllReferences, GoToDeclaration, GoToDefinition, GoToImplementation, GoToTypeDefinition,
};
use editor::hover_links::{HoverLink, OpenDefinitionLocations};
use editor::{Editor, EditorSettings, GotoDefinitionKind, OpenResultsIn};
use fuzzy::StringMatchCandidate;
use gpui::{
    AnyElement, App, AppContext, AsyncWindowContext, Bounds, Context, DismissEvent, DispatchPhase,
    Entity, EventEmitter, FocusHandle, Focusable, HighlightStyle, Pixels, StyledText, Subscription,
    Task, TextStyle, WeakEntity, point, prelude::*, size,
};
use language::{Buffer, CharKind, HighlightId, LanguageAwareStyling};
use picker::{Picker, PickerDelegate};
use project::{Location, LocationLink, Project, ProjectPath};
use settings::{GoToDefinitionFallback, Settings as _};
use text::{Anchor, Point};
use theme_settings::ThemeSettings;
use ui::{ListItem, ListItemSpacing, prelude::*};
use util::ResultExt as _;
use workspace::item::Item as _;
use workspace::notifications::NotificationId;
use workspace::pane::NavigationEntry;
use workspace::{ModalView, Toast, Workspace};

pub fn init(cx: &mut App) {
    cx.observe_new(register).detach();
}

/// Registers handlers for the navigation actions on each full editor. When the
/// action resolves to [`OpenResultsIn::Picker`], we open the filterable picker;
/// otherwise we `cx.propagate()` so the editor's own handler runs and builds a
/// multibuffer.
fn register(editor: &mut Editor, _window: Option<&mut Window>, cx: &mut Context<Editor>) {
    if !editor.mode().is_full() {
        return;
    }
    editor
        .register_action_renderer(|editor, window, _| {
            if editor.lsp_data_enabled() {
                window.on_action(
                    TypeId::of::<OpenDefinitionLocations>(),
                    |action, phase, window, cx| {
                        if phase == DispatchPhase::Bubble {
                            handle_definition_locations(
                                action.downcast_ref().expect("definition action"),
                                window,
                                cx,
                            );
                        }
                    },
                );
            }
        })
        .detach();
    let handle = cx.entity().downgrade();
    editor
        .register_action({
            let handle = handle.clone();
            move |action: &GoToDefinition, window, cx| {
                handle_nav_action(
                    action.open_results_in,
                    LspPickerKind::Definition,
                    &handle,
                    window,
                    cx,
                );
            }
        })
        .detach();
    editor
        .register_action({
            let handle = handle.clone();
            move |action: &GoToDeclaration, window, cx| {
                handle_nav_action(
                    action.open_results_in,
                    LspPickerKind::Declaration,
                    &handle,
                    window,
                    cx,
                );
            }
        })
        .detach();
    editor
        .register_action({
            let handle = handle.clone();
            move |action: &GoToImplementation, window, cx| {
                handle_nav_action(
                    action.open_results_in,
                    LspPickerKind::Implementation,
                    &handle,
                    window,
                    cx,
                );
            }
        })
        .detach();
    editor
        .register_action({
            let handle = handle.clone();
            move |action: &GoToTypeDefinition, window, cx| {
                handle_nav_action(
                    action.open_results_in,
                    LspPickerKind::TypeDefinition,
                    &handle,
                    window,
                    cx,
                );
            }
        })
        .detach();
    editor
        .register_action(move |action: &FindAllReferences, window, cx| {
            handle_nav_action(
                action.open_results_in,
                LspPickerKind::References,
                &handle,
                window,
                cx,
            );
        })
        .detach();
}

/// Either opens the picker for the editor, or propagates the action so the
/// editor's built-in (multibuffer) handler runs. A `None` argument falls back to
/// the `lsp_results_location` setting.
fn handle_nav_action(
    open_results_in: Option<OpenResultsIn>,
    kind: LspPickerKind,
    editor: &WeakEntity<Editor>,
    window: &mut Window,
    cx: &mut App,
) {
    let open_results_in =
        open_results_in.unwrap_or_else(|| EditorSettings::get_global(cx).lsp_results_location);
    if open_results_in != OpenResultsIn::Picker {
        cx.propagate();
        return;
    }
    LspLocationsPicker::open_for_editor(kind, editor.clone(), window, cx);
}

fn handle_definition_locations(
    action: &OpenDefinitionLocations,
    window: &mut Window,
    cx: &mut App,
) {
    let action = &action.0;
    let Some(editor) = action.editor.upgrade() else {
        return;
    };
    if !editor.read(cx).lsp_data_enabled()
        || !editor
            .read(cx)
            .buffer()
            .read(cx)
            .snapshot(cx)
            .can_resolve(&action.position)
    {
        return;
    }
    let Some(workspace) = editor.read(cx).workspace() else {
        cx.propagate();
        return;
    };
    if EditorSettings::get_global(cx).lsp_results_location != OpenResultsIn::Picker
        || action
            .locations
            .iter()
            .any(|location| location.buffer.read(cx).file().is_none())
    {
        cx.propagate();
        return;
    }
    let project = workspace.read(cx).project().read(cx);
    let matches = build_location_matches(&action.locations, project, cx);
    if matches.is_empty() && action.kind == GotoDefinitionKind::Implementation {
        workspace.update(cx, |workspace, cx| {
            show_no_results_toast(workspace, LspPickerKind::Implementation, cx)
        });
        return;
    }
    if matches.len() < 2 {
        cx.propagate();
        return;
    }
    let kind = match action.kind {
        GotoDefinitionKind::Symbol => LspPickerKind::Definition,
        GotoDefinitionKind::Type => LspPickerKind::TypeDefinition,
        GotoDefinitionKind::Declaration => LspPickerKind::Declaration,
        GotoDefinitionKind::Implementation => LspPickerKind::Implementation,
    };
    let anchor = cursor_line(&editor, window, cx);
    let symbol = word_at(&editor, action.position, cx);
    workspace.update(cx, |workspace, cx| {
        workspace.toggle_modal(window, cx, |window, cx| {
            LspLocationsPicker::new(
                kind,
                matches,
                symbol,
                action.editor.clone(),
                action.origin.clone(),
                anchor,
                window,
                cx,
            )
        });
    });
}

/// The cursor line in window coordinates, which the popup opens below.
fn cursor_line(editor: &Entity<Editor>, window: &Window, cx: &mut App) -> Option<Bounds<Pixels>> {
    let center = editor.read(cx).pixel_position_of_cursor(cx)?;
    let line_height = editor.update(cx, |editor, cx| {
        editor
            .style(cx)
            .text
            .line_height_in_pixels(window.rem_size())
    });
    Some(Bounds::new(
        point(center.x, center.y - line_height / 2.),
        size(px(0.), line_height),
    ))
}

/// The word at `position`, which names the symbol in the popup title.
fn word_at(editor: &Entity<Editor>, position: editor::Anchor, cx: &App) -> Option<String> {
    let snapshot = editor.read(cx).buffer().read(cx).snapshot(cx);
    let (range, kind) = snapshot.surrounding_word(position, None);
    (kind == Some(CharKind::Word)).then(|| snapshot.text_for_range(range).collect())
}

/// Runs the LSP query for `kind` and returns the raw locations. Returns `None`
/// (and reports any error) when there is nothing to query, so the caller stops
/// without an empty-results toast. Deduplication and dropping fileless results
/// happen later in [`build_location_matches`].
async fn run_picker_query(
    kind: LspPickerKind,
    editor: &WeakEntity<Editor>,
    workspace: &WeakEntity<Workspace>,
    project: &Entity<Project>,
    cx: &mut AsyncWindowContext,
) -> Option<Vec<Location>> {
    let query = editor
        .update(cx, |editor, cx| kind.run_query(editor, project, cx))
        .ok()
        .flatten()?;
    match query.await {
        Ok(locations) => Some(locations),
        Err(error) => {
            log::error!("LSP {kind:?} query failed: {error:#}");
            workspace
                .update(cx, |workspace, cx| workspace.show_error(error, cx))
                .log_err();
            None
        }
    }
}

/// Runs the query for `kind` and builds the displayable, deduped matches.
async fn run_picker_matches(
    kind: LspPickerKind,
    editor: &WeakEntity<Editor>,
    workspace: &WeakEntity<Workspace>,
    project: &Entity<Project>,
    cx: &mut AsyncWindowContext,
) -> Option<Vec<LocationMatch>> {
    let locations = run_picker_query(kind, editor, workspace, project, cx).await?;
    editor
        .update(cx, |_, cx| {
            build_location_matches(&locations, project.read(cx), cx)
        })
        .ok()
}

fn show_no_results_toast(
    workspace: &mut Workspace,
    kind: LspPickerKind,
    cx: &mut Context<Workspace>,
) {
    struct NoLspResults;
    workspace.show_toast(
        Toast::new(
            NotificationId::unique::<NoLspResults>(),
            kind.empty_message(),
        )
        .autohide(),
        cx,
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LspPickerKind {
    References,
    Definition,
    Declaration,
    Implementation,
    TypeDefinition,
}

impl LspPickerKind {
    /// The popup title, e.g. "Implementations of Foo (3)".
    fn title(self, symbol: Option<&str>, count: usize) -> String {
        let label = match self {
            LspPickerKind::References => "Usages",
            LspPickerKind::Definition => "Definitions",
            LspPickerKind::Declaration => "Declarations",
            LspPickerKind::Implementation => "Implementations",
            LspPickerKind::TypeDefinition => "Type definitions",
        };
        match symbol {
            Some(symbol) => format!("{label} of {symbol} ({count})"),
            None => format!("{label} ({count})"),
        }
    }

    /// Message shown when the query produces no results, so the command does not
    /// appear to silently do nothing.
    fn empty_message(self) -> &'static str {
        match self {
            LspPickerKind::References => "No references found",
            LspPickerKind::Definition => "No definitions found",
            LspPickerKind::Declaration => "No declarations found",
            LspPickerKind::Implementation => "No implementations found",
            LspPickerKind::TypeDefinition => "No type definitions found",
        }
    }

    /// Runs the query for this kind against the active editor, returning the raw
    /// locations to populate the picker.
    fn run_query(
        self,
        editor: &mut Editor,
        project: &Entity<Project>,
        cx: &mut Context<Editor>,
    ) -> Option<Task<anyhow::Result<Vec<Location>>>> {
        match self {
            LspPickerKind::References => editor.find_all_references_locations(project, cx),
            LspPickerKind::Definition => {
                editor.definition_locations_of_kind(GotoDefinitionKind::Symbol, cx)
            }
            LspPickerKind::Declaration => {
                editor.definition_locations_of_kind(GotoDefinitionKind::Declaration, cx)
            }
            LspPickerKind::Implementation => {
                editor.definition_locations_of_kind(GotoDefinitionKind::Implementation, cx)
            }
            LspPickerKind::TypeDefinition => {
                editor.definition_locations_of_kind(GotoDefinitionKind::Type, cx)
            }
        }
    }
}

pub struct LspLocationsPicker {
    picker: Entity<Picker<LspLocationsDelegate>>,
    anchor: Option<Bounds<Pixels>>,
    _subscription: Subscription,
}

impl LspLocationsPicker {
    fn open_for_editor(
        kind: LspPickerKind,
        editor: WeakEntity<Editor>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(editor) = editor.upgrade() else {
            return;
        };
        let Some(workspace) = editor.read(cx).workspace() else {
            return;
        };
        workspace.update(cx, |workspace, cx| {
            Self::open(kind, editor, workspace, window, cx);
        });
    }

    /// Opens the picker for `kind`: runs a fresh LSP query and shows the
    /// results. An empty definitions query falls back to references when the
    /// `go_to_definition_fallback` setting calls for it, matching
    /// [`Editor::go_to_definition`].
    fn open(
        kind: LspPickerKind,
        editor: Entity<Editor>,
        workspace: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let project = workspace.project().clone();
        let fallback = EditorSettings::get_global(cx).go_to_definition_fallback;
        let anchor = cursor_line(&editor, window, cx);
        let cursor = editor.read(cx).selections.newest_anchor().head();
        let symbol = word_at(&editor, cursor, cx);
        let editor = editor.downgrade();
        cx.spawn_in(window, async move |workspace, cx| {
            // The kind the user invoked, kept for user-facing messages even if
            // the query below falls back to references.
            let invoked_kind = kind;
            let mut kind = kind;

            // Count on the built matches (not raw locations): they are deduped by
            // range and exclude fileless results, so a single distinct result
            // jumps directly and a fileless-only result reports "no results"
            // instead of opening a blank picker.
            let Some(mut matches) =
                run_picker_matches(kind, &editor, &workspace, &project, cx).await
            else {
                return;
            };

            if matches.is_empty()
                && kind == LspPickerKind::Definition
                && fallback == GoToDefinitionFallback::FindAllReferences
            {
                kind = LspPickerKind::References;
                let Some(references) =
                    run_picker_matches(kind, &editor, &workspace, &project, cx).await
                else {
                    return;
                };
                matches = references;
            }

            if matches.is_empty() {
                workspace
                    .update(cx, |workspace, cx| {
                        show_no_results_toast(workspace, invoked_kind, cx)
                    })
                    .log_err();
                return;
            }

            if matches.len() == 1 {
                if let Some(location_match) = matches.into_iter().next() {
                    let location = Location {
                        buffer: location_match.buffer,
                        range: location_match.anchor_range,
                    };
                    if let Ok(task) = editor.update_in(cx, |editor, window, cx| {
                        editor.open_location(location, false, window, cx)
                    }) {
                        task.await.log_err();
                    }
                }
                return;
            }

            workspace
                .update_in(cx, |workspace, window, cx| {
                    workspace.toggle_modal(window, cx, |window, cx| {
                        Self::new(kind, matches, symbol, editor, None, anchor, window, cx)
                    });
                })
                .log_err();
        })
        .detach();
    }

    fn new(
        kind: LspPickerKind,
        matches: Vec<LocationMatch>,
        symbol: Option<String>,
        editor: WeakEntity<Editor>,
        origin: Option<NavigationEntry>,
        anchor: Option<Bounds<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let delegate = LspLocationsDelegate::new(kind, matches, symbol, editor, origin);
        let picker = cx.new(|cx| {
            Picker::uniform_list(delegate, window, cx)
                .popover()
                .initial_width(rems(48.))
        });
        let subscription = cx.subscribe(&picker, |_, _, _: &DismissEvent, cx| {
            cx.emit(DismissEvent);
        });
        Self {
            picker,
            anchor,
            _subscription: subscription,
        }
    }
}

impl ModalView for LspLocationsPicker {
    fn anchor(&self) -> Option<Bounds<Pixels>> {
        self.anchor
    }
}

impl EventEmitter<DismissEvent> for LspLocationsPicker {}

impl Focusable for LspLocationsPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl Render for LspLocationsPicker {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .debug_selector(|| "LSP_LOCATIONS_POPUP".into())
            .child(self.picker.clone())
    }
}

struct LocationMatch {
    path: ProjectPath,
    display_path: String,
    buffer: Entity<Buffer>,
    anchor_range: Range<Anchor>,
    range: Range<usize>,
    display_text: String,
    syntax_highlights: Vec<(Range<usize>, HighlightId)>,
    match_range: Range<usize>,
    line_number: u32,
}

struct LspLocationsDelegate {
    title: Arc<str>,
    editor: WeakEntity<Editor>,
    origin: Option<NavigationEntry>,
    all_matches: Vec<LocationMatch>,
    candidates: Arc<[StringMatchCandidate]>,
    matches: Vec<usize>,
    selected_index: usize,
}

impl LspLocationsDelegate {
    fn new(
        kind: LspPickerKind,
        all_matches: Vec<LocationMatch>,
        symbol: Option<String>,
        editor: WeakEntity<Editor>,
        origin: Option<NavigationEntry>,
    ) -> Self {
        // Match against the line text and the file path, mirroring the fuzzy
        // matching every other Zed picker uses.
        let candidates = all_matches
            .iter()
            .enumerate()
            .map(|(index, location_match)| {
                StringMatchCandidate::new(
                    index,
                    &format!(
                        "{} {}",
                        location_match.display_text, location_match.display_path
                    ),
                )
            })
            .collect();
        let matches = (0..all_matches.len()).collect();
        Self {
            title: kind.title(symbol.as_deref(), all_matches.len()).into(),
            editor,
            origin,
            all_matches,
            candidates,
            matches,
            selected_index: 0,
        }
    }

    fn selected_location_match(&self) -> Option<&LocationMatch> {
        self.all_matches
            .get(*self.matches.get(self.selected_index)?)
    }

    fn open_selected(&mut self, split: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(location_match) = self.selected_location_match() else {
            return;
        };
        let location = Location {
            buffer: location_match.buffer.clone(),
            range: location_match.anchor_range.clone(),
        };
        let Some(editor) = self.editor.upgrade() else {
            return;
        };
        editor
            .update(cx, |editor, cx| {
                if let Some(origin) = self.origin.take() {
                    editor.navigate_to_hover_links(
                        None,
                        vec![HoverLink::Text(LocationLink {
                            origin: None,
                            target: location,
                        })],
                        Some(origin),
                        split,
                        window,
                        cx,
                    )
                } else {
                    editor.open_location(location, split, window, cx)
                }
            })
            .detach_and_log_err(cx);
        cx.emit(DismissEvent);
    }
}

fn build_location_matches(
    locations: &[Location],
    project: &Project,
    cx: &App,
) -> Vec<LocationMatch> {
    use gpui::EntityId;
    let several_roots = project.visible_worktrees(cx).nth(1).is_some();
    let mut snapshots: HashMap<EntityId, language::BufferSnapshot> = HashMap::default();
    let mut matches = Vec::with_capacity(locations.len());

    for location in locations {
        let snapshot = snapshots
            .entry(location.buffer.entity_id())
            .or_insert_with(|| location.buffer.read(cx).snapshot());

        let Some(file) = snapshot.file() else {
            continue;
        };
        let path = ProjectPath {
            worktree_id: file.worktree_id(cx),
            path: file.path().clone(),
        };
        let in_single_root = !several_roots
            && project
                .worktree_for_id(path.worktree_id, cx)
                .is_some_and(|worktree| worktree.read(cx).is_visible());
        let display_path = if in_single_root {
            path.path.as_unix_str().to_string()
        } else {
            file.full_path(cx).to_string_lossy().into_owned()
        };

        let start_offset: usize = snapshot.summary_for_anchor(&location.range.start);
        let end_offset: usize = snapshot.summary_for_anchor(&location.range.end);
        let row = snapshot.offset_to_point(start_offset).row;
        let line_start = snapshot.point_to_offset(Point::new(row, 0));
        let line_end = snapshot.point_to_offset(Point::new(row, snapshot.line_len(row)));
        let full_line: String = snapshot.text_for_range(line_start..line_end).collect();

        // The row shows the line with leading indentation trimmed. Offsets below
        // are relative to that displayed text.
        let display_text = full_line.trim_start().to_string();
        let visible_start = line_end.saturating_sub(display_text.len());
        let visible_end = line_end;

        // Precompute syntax highlights for the displayed text so rendering a row
        // never re-snapshots the buffer or re-runs highlighting.
        let mut syntax_highlights = Vec::new();
        let mut offset = 0;
        for chunk in snapshot.chunks(
            visible_start..visible_end,
            LanguageAwareStyling {
                tree_sitter: true,
                diagnostics: false,
            },
        ) {
            let chunk_len = chunk.text.len();
            if let Some(id) = chunk.syntax_highlight_id {
                syntax_highlights.push((offset..offset + chunk_len, id));
            }
            offset += chunk_len;
        }

        // The match span, clamped into the displayed text. `clamp` bounds each
        // endpoint to the line; `min`/`max` then keep the range well-ordered even
        // for a malformed/inverted LSP range (clamping alone preserves bounds but
        // not `start <= end`).
        let clamped_start = start_offset.clamp(visible_start, visible_end) - visible_start;
        let clamped_end = end_offset.clamp(visible_start, visible_end) - visible_start;
        let match_range = clamped_start.min(clamped_end)..clamped_start.max(clamped_end);

        matches.push(LocationMatch {
            path,
            display_path,
            buffer: location.buffer.clone(),
            anchor_range: location.range.clone(),
            range: start_offset..end_offset,
            display_text,
            syntax_highlights,
            match_range,
            line_number: row + 1,
        });
    }

    // Group by file and order by position so the grouped display list is stable,
    // then drop exact-duplicate ranges a server may report more than once.
    matches.sort_by(|a, b| a.path.cmp(&b.path).then(a.range.start.cmp(&b.range.start)));
    matches.dedup_by(|a, b| a.path == b.path && a.range == b.range);
    matches
}

impl PickerDelegate for LspLocationsDelegate {
    type ListItem = AnyElement;

    fn name() -> &'static str {
        "lsp locations picker"
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        self.title.clone()
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn select_on_hover(&self) -> bool {
        false
    }

    fn set_selected_index(
        &mut self,
        ix: usize,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) {
        self.selected_index = ix;
    }

    fn update_matches(
        &mut self,
        query: String,
        _window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        let query = query.trim().to_owned();
        let candidates = self.candidates.clone();
        cx.spawn(async move |picker, cx| {
            let matches = if query.is_empty() {
                (0..candidates.len()).collect()
            } else {
                let string_matches = fuzzy::match_strings(
                    &candidates,
                    &query,
                    false,
                    true,
                    candidates.len(),
                    &Default::default(),
                    cx.background_executor().clone(),
                )
                .await;
                let mut indices = string_matches
                    .into_iter()
                    .map(|string_match| string_match.candidate_id)
                    .collect::<Vec<_>>();
                // Keep the file and position order; fuzzy matching returns by score.
                indices.sort_unstable();
                indices
            };
            picker
                .update(cx, |picker, cx| {
                    picker.delegate.matches = matches;
                    picker.delegate.selected_index = 0;
                    cx.notify();
                })
                .ok();
        })
    }

    fn confirm(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.open_selected(secondary, window, cx);
    }

    fn dismissed(&mut self, _window: &mut Window, cx: &mut Context<Picker<Self>>) {
        cx.emit(DismissEvent);
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let location_match = self.all_matches.get(*self.matches.get(ix)?)?;
        Some(
            ListItem::new(ix)
                .spacing(ListItemSpacing::Sparse)
                .inset(true)
                .toggle_state(selected)
                .child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .gap_2()
                        .text_sm()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(render_matched_line(location_match, cx)),
                        )
                        .child(
                            div().flex_none().max_w(relative(0.5)).child(
                                Label::new(format!(
                                    "{}:{}",
                                    location_match.display_path, location_match.line_number
                                ))
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .truncate_start(),
                            ),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// Renders the precomputed displayed line, resolving the stored syntax highlight
/// ids against the current theme and overlaying the match with a highlighted
/// background and bold weight.
fn render_matched_line(location_match: &LocationMatch, cx: &App) -> StyledText {
    let settings = ThemeSettings::get_global(cx);
    let text_style = TextStyle {
        color: cx.theme().colors().text,
        font_family: settings.buffer_font.family.clone(),
        font_features: settings.buffer_font.features.clone(),
        font_fallbacks: settings.buffer_font.fallbacks.clone(),
        font_size: settings.buffer_font_size(cx).into(),
        font_weight: settings.buffer_font.weight,
        line_height: relative(1.),
        ..Default::default()
    };

    let syntax_theme = cx.theme().syntax();
    let syntax_highlights = location_match
        .syntax_highlights
        .iter()
        .filter_map(|(range, id)| Some((range.clone(), syntax_theme.get(*id).copied()?)))
        .collect::<Vec<_>>();

    let match_style = HighlightStyle {
        background_color: Some(cx.theme().colors().search_match_background),
        font_weight: Some(gpui::FontWeight::BOLD),
        ..Default::default()
    };
    let match_highlight = (location_match.match_range.clone(), match_style);

    let highlights = gpui::combine_highlights(syntax_highlights, [match_highlight]);
    StyledText::new(location_match.display_text.clone())
        .with_default_highlights(&text_style, highlights)
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::test::editor_lsp_test_context::EditorLspTestContext;
    use gpui::{Modifiers, TestAppContext};
    use indoc::indoc;
    use project::Fs as _;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::{sync::Mutex, time::Duration};
    use text::OffsetRangeExt as _;

    async fn rust_cx(
        capabilities: lsp::ServerCapabilities,
        cx: &mut TestAppContext,
    ) -> EditorLspTestContext {
        EditorLspTestContext::new_rust(capabilities, cx).await
    }

    fn open(cx: &mut EditorLspTestContext, kind: LspPickerKind) {
        let editor = cx.editor.clone();
        let workspace = cx.workspace.clone();
        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                LspLocationsPicker::open(kind, editor, workspace, window, cx);
            });
        });
        cx.run_until_parked();
    }

    fn active_picker(cx: &mut EditorLspTestContext) -> Option<Entity<LspLocationsPicker>> {
        let workspace = cx.workspace.clone();
        cx.update(|_window, cx| workspace.read(cx).active_modal::<LspLocationsPicker>(cx))
    }

    fn references(uri: lsp::Uri, ranges: &[(u32, u32, u32)]) -> Vec<lsp::Location> {
        ranges
            .iter()
            .map(|&(row, start, end)| lsp::Location {
                uri: uri.clone(),
                range: lsp::Range::new(
                    lsp::Position::new(row, start),
                    lsp::Position::new(row, end),
                ),
            })
            .collect()
    }

    const SOURCE: &str = indoc! {r#"
        fn main() {
            let aˇbc = 123;
            let xyz = abc;
        }
    "#};

    #[gpui::test]
    async fn test_multiple_references_open_picker(cx: &mut TestAppContext) {
        let mut cx = rust_cx(
            lsp::ServerCapabilities {
                references_provider: Some(lsp::OneOf::Left(true)),
                ..Default::default()
            },
            cx,
        )
        .await;
        cx.set_state(SOURCE);
        cx.lsp
            .set_request_handler::<lsp::request::References, _, _>(async move |params, _| {
                let uri = params.text_document_position.text_document.uri;
                Ok(Some(references(uri, &[(1, 8, 11), (2, 14, 17)])))
            });

        open(&mut cx, LspPickerKind::References);

        assert!(
            active_picker(&mut cx).is_some(),
            "multiple references should open the picker"
        );
    }

    #[gpui::test]
    async fn test_single_result_jumps_without_picker(cx: &mut TestAppContext) {
        let mut cx = rust_cx(
            lsp::ServerCapabilities {
                references_provider: Some(lsp::OneOf::Left(true)),
                ..Default::default()
            },
            cx,
        )
        .await;
        cx.set_state(SOURCE);
        cx.lsp
            .set_request_handler::<lsp::request::References, _, _>(async move |params, _| {
                let uri = params.text_document_position.text_document.uri;
                Ok(Some(references(uri, &[(2, 14, 17)])))
            });

        open(&mut cx, LspPickerKind::References);

        assert!(
            active_picker(&mut cx).is_none(),
            "a single result should jump directly instead of opening the picker"
        );
        // The lone result at row 2 should be selected directly, moving the
        // cursor off its starting position on row 1.
        cx.assert_editor_state(indoc! {r#"
            fn main() {
                let abc = 123;
                let xyz = «abcˇ»;
            }
        "#});
    }

    #[gpui::test]
    async fn test_no_results_does_not_open_picker(cx: &mut TestAppContext) {
        let mut cx = rust_cx(
            lsp::ServerCapabilities {
                references_provider: Some(lsp::OneOf::Left(true)),
                ..Default::default()
            },
            cx,
        )
        .await;
        cx.set_state(SOURCE);
        cx.lsp
            .set_request_handler::<lsp::request::References, _, _>(async move |_params, _| {
                Ok(Some(Vec::new()))
            });

        open(&mut cx, LspPickerKind::References);

        assert!(
            active_picker(&mut cx).is_none(),
            "an empty result should not open the picker"
        );
    }

    #[gpui::test]
    async fn test_definition_falls_back_to_references_picker(cx: &mut TestAppContext) {
        let mut cx = rust_cx(
            lsp::ServerCapabilities {
                definition_provider: Some(lsp::OneOf::Left(true)),
                references_provider: Some(lsp::OneOf::Left(true)),
                ..Default::default()
            },
            cx,
        )
        .await;
        cx.set_state(SOURCE);
        cx.lsp
            .set_request_handler::<lsp::request::GotoDefinition, _, _>(async move |_params, _| {
                Ok(None)
            });
        cx.lsp
            .set_request_handler::<lsp::request::References, _, _>(async move |params, _| {
                let uri = params.text_document_position.text_document.uri;
                Ok(Some(references(uri, &[(1, 8, 11), (2, 14, 17)])))
            });

        open(&mut cx, LspPickerKind::Definition);

        assert!(
            active_picker(&mut cx).is_some(),
            "an empty definition query should fall back to the references picker"
        );
    }

    #[gpui::test]
    async fn test_fuzzy_filter_matches_subsequence(cx: &mut TestAppContext) {
        let mut cx = rust_cx(
            lsp::ServerCapabilities {
                references_provider: Some(lsp::OneOf::Left(true)),
                ..Default::default()
            },
            cx,
        )
        .await;
        cx.set_state(SOURCE);
        cx.lsp
            .set_request_handler::<lsp::request::References, _, _>(async move |params, _| {
                let uri = params.text_document_position.text_document.uri;
                Ok(Some(references(uri, &[(1, 8, 11), (2, 14, 17)])))
            });

        open(&mut cx, LspPickerKind::References);
        let modal = active_picker(&mut cx).expect("multiple references should open the picker");
        let picker = cx.update(|_window, cx| modal.read(cx).picker.clone());

        let matches = |cx: &mut EditorLspTestContext, query: &str| -> usize {
            cx.update(|window, cx| {
                picker.update(cx, |picker, cx| picker.set_query(query, window, cx));
            });
            cx.run_until_parked();
            cx.update(|_window, cx| picker.read(cx).delegate.matches.len())
        };

        // "lx" is a subsequence of "let xyz" but not a substring of either line,
        // so it only matches with fuzzy matching.
        assert_eq!(matches(&mut cx, "lx"), 1);
        assert_eq!(matches(&mut cx, "zzzz"), 0);
        assert_eq!(matches(&mut cx, ""), 2);
    }

    #[gpui::test]
    async fn test_cmd_click_fallback_honors_lsp_results_location(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let mut cx = rust_cx(
            lsp::ServerCapabilities {
                definition_provider: Some(lsp::OneOf::Left(true)),
                references_provider: Some(lsp::OneOf::Left(true)),
                ..Default::default()
            },
            cx,
        )
        .await;
        cx.update(|_window, cx| {
            cx.update_global::<settings::SettingsStore, _>(|settings, cx| {
                settings.update_user_settings(cx, |settings| {
                    settings.editor.lsp_results_location = Some(OpenResultsIn::Picker);
                });
            });
        });
        cx.set_state(SOURCE);
        cx.lsp
            .set_request_handler::<lsp::request::GotoDefinition, _, _>(async move |_params, _| {
                Ok(None)
            });
        cx.lsp
            .set_request_handler::<lsp::request::References, _, _>(async move |params, _| {
                let uri = params.text_document_position.text_document.uri;
                Ok(Some(references(uri, &[(1, 8, 11), (2, 14, 17)])))
            });

        let screen_coord = cx
            .editor(|editor, _, cx| editor.pixel_position_of_cursor(cx))
            .unwrap();
        cx.simulate_click(screen_coord, gpui::Modifiers::secondary_key());
        cx.run_until_parked();

        assert!(
            active_picker(&mut cx).is_some(),
            "the cmd-click go-to-definition fallback should open the references picker when lsp_results_location is picker"
        );
    }

    #[gpui::test]
    async fn test_cmd_click_cold_locations(cx: &mut TestAppContext) {
        cx.update(crate::init);
        for results in [OpenResultsIn::Picker, OpenResultsIn::MultiBuffer] {
            for shift in [false, true] {
                assert_cmd_click(cx, None, shift, false, results, false).await;
            }
            assert_cmd_click(cx, None, false, false, results, true).await;
        }
    }

    #[gpui::test]
    async fn test_cmd_click_cached_locations(cx: &mut TestAppContext) {
        cx.update(crate::init);
        for results in [OpenResultsIn::Picker, OpenResultsIn::MultiBuffer] {
            for shift in [false, true] {
                assert_cmd_click(cx, Some(shift), shift, false, results, false).await;
                for pending in [false, true] {
                    assert_cmd_click(cx, Some(!shift), shift, pending, results, false).await;
                }
            }
        }
    }

    #[gpui::test]
    async fn test_cmd_click_empty_hover_requeries(cx: &mut TestAppContext) {
        cx.update(crate::init);
        for count in [1, 0, 2] {
            let targets = vec![(2, 14, 17); count];
            let (mut cx, requests) = cmd_click_request(cx, true, &targets).await;
            let target = SOURCE.replace('ˇ', "").replace("= abc", "= «abcˇ»");
            cx.assert_editor_state(if count == 0 { SOURCE } else { &target });
            assert!(active_picker(&mut cx).is_none());
            cx.update_workspace(|workspace, _, cx| assert_eq!(workspace.items(cx).count(), 1));
            assert_eq!(requests.load(Ordering::SeqCst), 2);
            cx.update(|window, _| window.remove_window());
            cx.run_until_parked();
        }
    }

    #[gpui::test]
    async fn test_cmd_click_pending_hidden_source(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (mut cx, requests) = cmd_click_request(cx, false, &[(2, 14, 17)]).await;
        let other = cx
            .update_workspace(Editor::new_in_workspace)
            .await
            .expect("tab");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update_workspace(|workspace, _, cx| {
            assert_eq!(workspace.active_item_as::<Editor>(cx), Some(other.clone()));
        });
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        cx.update_workspace(|workspace, _, cx| {
            assert_eq!(workspace.active_item_as::<Editor>(cx), Some(other));
        });
        cx.assert_editor_state(&SOURCE.replace('ˇ', "").replace("= abc", "= «abcˇ»"));
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[gpui::test]
    async fn test_cmd_click_pending_preserves_tag_origin(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (mut cx, requests) = cmd_click_request(cx, false, &[(0, 3, 7), (2, 14, 17)]).await;
        let moved = format!("ˇ{}", SOURCE.replace('ˇ', ""));
        cx.set_selections_state(&moved);
        assert!(active_picker(&mut cx).is_none());
        cx.assert_editor_state(&moved);
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        cx.assert_editor_state(&moved);
        let modal = active_picker(&mut cx).expect("locations picker");
        cx.update(|window, cx| {
            modal.read(cx).picker.clone().update(cx, |picker, cx| {
                let matches = &picker.delegate.all_matches;
                let ranges = matches.iter().map(|location| location.range.clone());
                assert_eq!(ranges.collect::<Vec<_>>(), vec![3..7, 45..48]);
                picker.delegate.confirm(false, window, cx);
            });
        });
        cx.run_until_parked();
        cx.assert_editor_state(&SOURCE.replace('ˇ', "").replace("main", "«mainˇ»"));
        cx.dispatch_action(workspace::ReopenLastPicker);
        cx.run_until_parked();
        let modal = active_picker(&mut cx).expect("reopened locations picker");
        cx.update(|window, cx| {
            modal.read(cx).picker.clone().update(cx, |picker, cx| {
                picker.set_selected_index(1, None, true, window, cx);
                picker.delegate.confirm(false, window, cx);
            });
        });
        cx.run_until_parked();
        cx.assert_editor_state(&SOURCE.replace('ˇ', "").replace("= abc", "= «abcˇ»"));
        cx.dispatch_action(workspace::pane::GoToOlderTag);
        cx.assert_editor_state(&SOURCE.replace('ˇ', "").replace("main", "mainˇ"));
        cx.dispatch_action(workspace::pane::GoToOlderTag);
        cx.assert_editor_state(SOURCE);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[gpui::test]
    async fn test_type_definition_honors_lsp_results_location(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let mut cx = rust_cx(
            lsp::ServerCapabilities {
                type_definition_provider: Some(lsp::TypeDefinitionProviderCapability::Simple(true)),
                ..Default::default()
            },
            cx,
        )
        .await;
        cx.update(|_window, cx| {
            cx.update_global::<settings::SettingsStore, _>(|settings, cx| {
                settings.update_user_settings(cx, |settings| {
                    settings.editor.lsp_results_location = Some(OpenResultsIn::Picker);
                });
            });
        });
        cx.set_state(indoc! {r#"
            fn main() {
                struct Foo<T>(T);
                struct Bar;
                let fˇoo: Foo<Bar>;
            }
        "#});
        cx.lsp
            .set_request_handler::<lsp::request::GotoTypeDefinition, _, _>(
                async move |params, _| {
                    let uri = params.text_document_position_params.text_document.uri;
                    Ok(Some(lsp::GotoDefinitionResponse::Array(references(
                        uri,
                        &[(1, 11, 14), (2, 11, 14)],
                    ))))
                },
            );

        cx.dispatch_action(GoToTypeDefinition::default());

        assert!(
            active_picker(&mut cx).is_some(),
            "type definition should open the picker when lsp_results_location is picker"
        );
    }

    #[gpui::test]
    async fn test_declaration_honors_lsp_results_location(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let mut cx = rust_cx(
            lsp::ServerCapabilities {
                declaration_provider: Some(lsp::DeclarationCapability::Simple(true)),
                ..Default::default()
            },
            cx,
        )
        .await;
        cx.update(|_window, cx| {
            cx.update_global::<settings::SettingsStore, _>(|settings, cx| {
                settings.update_user_settings(cx, |settings| {
                    settings.editor.lsp_results_location = Some(OpenResultsIn::Picker);
                });
            });
        });
        cx.set_state(indoc! {r#"
            fn main() {
                let foo = ();
                let foo = ();
                let bar = fˇoo;
            }
        "#});
        cx.lsp
            .set_request_handler::<lsp::request::GotoDeclaration, _, _>(async move |params, _| {
                let uri = params.text_document_position_params.text_document.uri;
                Ok(Some(lsp::GotoDefinitionResponse::Array(references(
                    uri,
                    &[(1, 8, 11), (2, 8, 11)],
                ))))
            });

        cx.dispatch_action(GoToDeclaration::default());

        assert!(
            active_picker(&mut cx).is_some(),
            "declaration should open the picker when lsp_results_location is picker"
        );
    }

    async fn assert_cmd_click(
        cx: &mut TestAppContext,
        hover: Option<bool>,
        shift: bool,
        pending: bool,
        results: OpenResultsIn,
        disabled: bool,
    ) {
        let mut cx = rust_cx(
            lsp::ServerCapabilities {
                definition_provider: Some(lsp::OneOf::Left(true)),
                type_definition_provider: Some(lsp::TypeDefinitionProviderCapability::Simple(true)),
                ..lsp::ServerCapabilities::default()
            },
            cx,
        )
        .await;
        let scenario = format!(
            "hover={hover:?}, shift={shift}, pending={pending}, results={results:?}, disabled={disabled}"
        );
        cx.update_global::<settings::SettingsStore, _>(|settings, cx| {
            settings.update_user_settings(cx, |settings| {
                settings.editor.lsp_results_location = Some(results);
                settings.editor.excerpt_context_lines = Some(0);
            });
        });
        let source = indoc! {r#"
            fn main() {
                let foo = ();

                let foo = ();
                let bar = fˇoo;
            }
        "#}
        .replace("\n    let bar", "\n\n\n\n\n\n    let bar");
        cx.set_state(&source);
        if disabled {
            cx.update_editor(|editor, _, _| {
                editor.set_mode(editor::EditorMode::AutoHeight {
                    min_lines: 1,
                    max_lines: None,
                })
            });
        }
        let position = cx
            .editor(|editor, _, cx| editor.pixel_position_of_cursor(cx))
            .expect("click pixel");
        cx.set_selections_state(&source.replace('ˇ', "").replace("main", "ˇmain"));
        cx.assert_editor_selections(vec![3..3]);
        let buffer = cx.multibuffer(|buffer, _| buffer.as_singleton().expect("source buffer"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let completed = Arc::new(AtomicUsize::new(0));
        let delay = Duration::from_secs(1);
        for kind in [false, true] {
            let requests = requests.clone();
            let completed = completed.clone();
            let handler = move |params: lsp::GotoDefinitionParams, app: gpui::AsyncApp| {
                let requests = requests.clone();
                let completed = completed.clone();
                async move {
                    let params = params.text_document_position_params;
                    requests.lock().expect("lock").push((kind, params.clone()));
                    if pending && kind == shift {
                        app.background_executor().timer(delay).await;
                    }
                    completed.fetch_add(1, Ordering::SeqCst);
                    let (start, end) = if kind { (14, 16) } else { (8, 11) };
                    Ok(Some(lsp::GotoDefinitionResponse::Array(references(
                        params.text_document.uri,
                        &[(1, start, end), (3, start, end)],
                    ))))
                }
            };
            if kind {
                cx.lsp
                    .set_request_handler::<lsp::request::GotoTypeDefinition, _, _>(handler);
            } else {
                cx.lsp
                    .set_request_handler::<lsp::request::GotoDefinition, _, _>(handler);
            }
        }
        let modifiers = Modifiers {
            shift,
            ..Modifiers::secondary_key()
        };
        let mut expected_requests = Vec::new();
        let query = lsp::TextDocumentPositionParams {
            text_document: lsp::TextDocumentIdentifier::new(cx.buffer_lsp_url.clone()),
            position: lsp::Position::new(9, 15),
        };
        if let Some(shift) = hover {
            let mut modifiers = modifiers;
            modifiers.shift = shift;
            cx.simulate_mouse_move(position, None, modifiers);
            expected_requests.push((shift, query.clone()));
        }
        if pending {
            cx.simulate_mouse_move(position, None, modifiers);
            expected_requests.push((shift, query.clone()));
        }
        assert_eq!(*requests.lock().expect("requests"), expected_requests);
        assert_eq!(
            completed.load(Ordering::SeqCst),
            usize::from(hover.is_some())
        );
        cx.simulate_click(position, modifiers);
        if pending {
            cx.executor().advance_clock(delay);
        }
        cx.run_until_parked();
        if !disabled && hover != Some(shift) {
            expected_requests.push((shift, query));
        }
        assert_eq!(
            *requests.lock().expect("requests"),
            expected_requests,
            "{scenario}"
        );
        let picker = active_picker(&mut cx);
        let source_editor = cx.editor.clone();
        let (start, end) = if shift { (14, 16) } else { (8, 11) };
        let expected_ranges = vec![
            Point::new(1, start)..Point::new(1, end),
            Point::new(3, start)..Point::new(3, end),
        ];
        cx.update_workspace(|workspace, _, cx| {
            assert_eq!(workspace.panes().len(), 1);
            let active = workspace.active_item_as::<Editor>(cx).expect("editor");
            if disabled {
                assert!(picker.is_none());
                assert_eq!(active, source_editor);
                assert_eq!(workspace.items(cx).count(), 1);
                return;
            }
            let actual_ranges = if results == OpenResultsIn::Picker {
                let picker = picker.expect("locations picker");
                let delegate = &picker.read(cx).picker.read(cx).delegate;
                let title_prefix = match shift {
                    true => "Type definitions",
                    false => "Definitions",
                };
                assert!(
                    delegate.title.starts_with(title_prefix),
                    "unexpected title {:?}",
                    delegate.title
                );
                assert_eq!(workspace.items(cx).count(), 1);
                delegate
                    .all_matches
                    .iter()
                    .map(|location| {
                        assert_eq!(location.buffer, buffer);
                        location.anchor_range.to_point(buffer.read(cx))
                    })
                    .collect::<Vec<_>>()
            } else {
                assert!(picker.is_none());
                let multibuffer = active.read(cx).buffer().read(cx);
                assert!(!multibuffer.is_singleton());
                assert_eq!(
                    multibuffer.all_buffers_iter().collect::<Vec<_>>(),
                    vec![buffer.clone()]
                );
                assert_eq!(workspace.items(cx).count(), 2);
                multibuffer
                    .snapshot(cx)
                    .excerpts()
                    .map(|excerpt| excerpt.primary.to_point(buffer.read(cx)))
                    .collect::<Vec<_>>()
            };
            assert_eq!(actual_ranges, expected_ranges);
        });
        cx.update(|window, _| window.remove_window());
        cx.run_until_parked();
    }

    async fn cmd_click_request(
        cx: &mut TestAppContext,
        empty_hover: bool,
        targets: &[(u32, u32, u32)],
    ) -> (EditorLspTestContext, Arc<AtomicUsize>) {
        let mut cx = rust_cx(
            lsp::ServerCapabilities {
                definition_provider: Some(lsp::OneOf::Left(true)),
                ..lsp::ServerCapabilities::default()
            },
            cx,
        )
        .await;
        cx.update_global::<settings::SettingsStore, _>(|settings, cx| {
            settings.update_user_settings(cx, |settings| {
                settings.editor.lsp_results_location = Some(OpenResultsIn::Picker);
                settings.editor.go_to_definition_fallback = Some(GoToDefinitionFallback::None);
            });
        });
        cx.set_state(SOURCE);
        let requests = Arc::new(AtomicUsize::new(0));
        let locations = references(cx.buffer_lsp_url.clone(), targets);
        cx.set_request_handler::<lsp::request::GotoDefinition, _, _>({
            let requests = requests.clone();
            move |uri, params, app| {
                let params = params.text_document_position_params;
                assert_eq!(params.text_document.uri, uri);
                assert_eq!(params.position, lsp::Position::new(1, 9));
                let empty = requests.fetch_add(1, Ordering::SeqCst) == 0 && empty_hover;
                let locations = locations.clone();
                async move {
                    if !empty_hover {
                        app.background_executor()
                            .timer(Duration::from_secs(1))
                            .await;
                    }
                    Ok(Some(lsp::GotoDefinitionResponse::Array(if empty {
                        Vec::new()
                    } else {
                        locations
                    })))
                }
            }
        });
        let position = cx
            .editor(|editor, _, cx| editor.pixel_position_of_cursor(cx))
            .expect("click pixel");
        if empty_hover {
            cx.simulate_mouse_move(position, None, Modifiers::secondary_key());
        }
        cx.simulate_click(position, Modifiers::secondary_key());
        cx.simulate_modifiers_change(Modifiers::none());
        (cx, requests)
    }

    async fn references_cx(cx: &mut TestAppContext) -> EditorLspTestContext {
        cx.update(crate::init);
        let mut cx = rust_cx(
            lsp::ServerCapabilities {
                references_provider: Some(lsp::OneOf::Left(true)),
                ..Default::default()
            },
            cx,
        )
        .await;
        cx.set_state(SOURCE);
        cx.lsp
            .set_request_handler::<lsp::request::References, _, _>(async move |params, _| {
                let uri = params.text_document_position.text_document.uri;
                Ok(Some(references(uri, &[(1, 8, 11), (2, 14, 17)])))
            });
        cx
    }

    #[gpui::test]
    async fn test_popup_is_the_default(cx: &mut TestAppContext) {
        let mut cx = references_cx(cx).await;
        cx.update(|_window, cx| {
            assert_eq!(
                EditorSettings::get_global(cx).lsp_results_location,
                OpenResultsIn::Picker
            );
        });

        cx.dispatch_action(FindAllReferences::default());
        cx.run_until_parked();

        assert!(
            active_picker(&mut cx).is_some(),
            "references should open the popup by default"
        );
    }

    #[gpui::test]
    async fn test_multi_buffer_argument_opens_results_tab(cx: &mut TestAppContext) {
        let mut cx = references_cx(cx).await;

        cx.dispatch_action(FindAllReferences {
            open_results_in: Some(OpenResultsIn::MultiBuffer),
            ..FindAllReferences::default()
        });
        cx.run_until_parked();

        assert!(active_picker(&mut cx).is_none());
        let workspace = cx.workspace.clone();
        cx.update(|_window, cx| {
            let active_editor = workspace
                .read(cx)
                .active_item_as::<Editor>(cx)
                .expect("an active editor");
            assert!(
                !active_editor.read(cx).buffer().read(cx).is_singleton(),
                "the references should open in a multibuffer tab"
            );
        });
    }

    #[gpui::test]
    async fn test_popup_is_anchored_at_the_cursor_line(cx: &mut TestAppContext) {
        let mut cx = references_cx(cx).await;
        let editor = cx.editor.clone();

        open(&mut cx, LspPickerKind::References);

        let picker = active_picker(&mut cx).expect("references popup");
        cx.update(|_window, cx| {
            let cursor = editor
                .read(cx)
                .pixel_position_of_cursor(cx)
                .expect("painted cursor");
            let line = picker.read(cx).anchor.expect("anchored popup");
            assert_eq!(line.origin.x, cursor.x);
            assert_eq!(line.origin.y + line.size.height / 2., cursor.y);
        });
    }

    #[gpui::test]
    async fn test_title_names_the_symbol_and_the_count(cx: &mut TestAppContext) {
        let mut cx = references_cx(cx).await;

        open(&mut cx, LspPickerKind::References);

        let picker = active_picker(&mut cx).expect("references popup");
        cx.update(|_window, cx| {
            assert_eq!(
                picker.read(cx).picker.read(cx).delegate.title.as_ref(),
                "Usages of abc (2)"
            );
        });
    }

    /// Opens the references popup on results in two same-named files and in a
    /// file outside the project, and returns the paths its rows show.
    async fn displayed_paths(
        cx: &mut TestAppContext,
        extra_root: Option<&'static str>,
    ) -> Vec<String> {
        let mut cx = references_cx(cx).await;
        let project = cx.update_workspace(|workspace, _, _| workspace.project().clone());
        let fs = cx.update(|_, cx| project.read(cx).fs().as_fake());
        for directory in ["/root/a", "/root/b", "/outside"]
            .into_iter()
            .chain(extra_root)
        {
            fs.create_dir(Path::new(directory))
                .await
                .expect("directory");
        }
        let content = "fn abc() {}\n";
        fs.insert_file("/root/a/mod.rs", content.into()).await;
        fs.insert_file("/root/b/mod.rs", content.into()).await;
        fs.insert_file("/outside/lib.rs", content.into()).await;
        if let Some(root) = extra_root {
            fs.insert_file(format!("{root}/main.rs"), content.into())
                .await;
            project
                .update(&mut cx.cx.cx, |project, cx| {
                    project.find_or_create_worktree(root, true, cx)
                })
                .await
                .expect("extra root");
        }
        cx.run_until_parked();
        cx.lsp
            .set_request_handler::<lsp::request::References, _, _>(async move |_, _| {
                Ok(Some(
                    ["/root/a/mod.rs", "/root/b/mod.rs", "/outside/lib.rs"]
                        .into_iter()
                        .flat_map(|path| {
                            let uri = lsp::Uri::from_file_path(path).expect("absolute path");
                            references(uri, &[(0, 3, 6)])
                        })
                        .collect(),
                ))
            });

        open(&mut cx, LspPickerKind::References);

        let picker = active_picker(&mut cx).expect("references popup");
        cx.update(|_window, cx| {
            let mut paths: Vec<String> = picker
                .read(cx)
                .picker
                .read(cx)
                .delegate
                .all_matches
                .iter()
                .map(|location_match| location_match.display_path.clone())
                .collect();
            paths.sort();
            paths
        })
    }

    #[gpui::test]
    async fn test_rows_show_the_path_from_the_project_root(cx: &mut TestAppContext) {
        assert_eq!(
            displayed_paths(cx, None).await,
            ["/outside/lib.rs", "a/mod.rs", "b/mod.rs"]
        );
    }

    #[gpui::test]
    async fn test_rows_name_the_root_when_the_project_has_several(cx: &mut TestAppContext) {
        assert_eq!(
            displayed_paths(cx, Some("/other")).await,
            ["/outside/lib.rs", "root/a/mod.rs", "root/b/mod.rs"]
        );
    }

    #[gpui::test]
    async fn test_popup_opens_wide(cx: &mut TestAppContext) {
        let mut cx = references_cx(cx).await;

        open(&mut cx, LspPickerKind::References);

        let bounds = cx
            .debug_bounds("LSP_LOCATIONS_POPUP")
            .expect("rendered popup");
        let minimum = cx.update(|window, _| rems(48.).to_pixels(window.rem_size()));
        assert!(
            bounds.size.width >= minimum,
            "popup is {:?} wide, expected at least {minimum:?}",
            bounds.size.width
        );
    }

    /// Ctrl+Alt+clicks `abc` while the server reports `targets` as its
    /// implementations, and returns the LSP methods it was asked for.
    async fn implementation_click(
        cx: &mut TestAppContext,
        results: OpenResultsIn,
        targets: &'static [(u32, u32, u32)],
    ) -> (EditorLspTestContext, Arc<Mutex<Vec<&'static str>>>) {
        cx.update(crate::init);
        let mut cx = rust_cx(
            lsp::ServerCapabilities {
                definition_provider: Some(lsp::OneOf::Left(true)),
                implementation_provider: Some(lsp::ImplementationProviderCapability::Simple(true)),
                references_provider: Some(lsp::OneOf::Left(true)),
                ..Default::default()
            },
            cx,
        )
        .await;
        cx.update_global::<settings::SettingsStore, _>(|settings, cx| {
            settings.update_user_settings(cx, |settings| {
                settings.editor.lsp_results_location = Some(results);
            });
        });
        cx.set_state(SOURCE);
        let requests = Arc::new(Mutex::new(Vec::new()));
        cx.lsp
            .set_request_handler::<lsp::request::GotoImplementation, _, _>({
                let requests = requests.clone();
                move |params, _| {
                    requests.lock().expect("requests").push("implementation");
                    let uri = params.text_document_position_params.text_document.uri;
                    async move {
                        Ok(Some(lsp::GotoDefinitionResponse::Array(references(
                            uri, targets,
                        ))))
                    }
                }
            });
        cx.lsp
            .set_request_handler::<lsp::request::References, _, _>({
                let requests = requests.clone();
                move |_, _| {
                    requests.lock().expect("requests").push("references");
                    async move { Ok(Some(Vec::new())) }
                }
            });
        let position = cx
            .editor(|editor, _, cx| editor.pixel_position_of_cursor(cx))
            .expect("click pixel");
        let modifiers = Modifiers {
            alt: true,
            ..Modifiers::secondary_key()
        };
        cx.simulate_mouse_move(position, None, modifiers);
        cx.simulate_click(position, modifiers);
        cx.run_until_parked();
        (cx, requests)
    }

    #[gpui::test]
    async fn test_ctrl_alt_click_opens_implementations(cx: &mut TestAppContext) {
        let (mut cx, requests) =
            implementation_click(cx, OpenResultsIn::Picker, &[(0, 3, 7), (2, 14, 17)]).await;

        assert_eq!(*requests.lock().expect("requests"), ["implementation"]);
        let picker = active_picker(&mut cx).expect("implementations popup");
        cx.update(|_window, cx| {
            assert_eq!(
                picker.read(cx).picker.read(cx).delegate.title.as_ref(),
                "Implementations of abc (2)"
            );
        });
    }

    #[gpui::test]
    async fn test_ctrl_alt_click_jumps_to_a_single_implementation(cx: &mut TestAppContext) {
        let (mut cx, _) = implementation_click(cx, OpenResultsIn::Picker, &[(2, 14, 17)]).await;

        assert!(active_picker(&mut cx).is_none());
        cx.update_editor(|editor, _, cx| {
            let snapshot = editor.display_snapshot(cx);
            assert_eq!(editor.selections.newest::<Point>(&snapshot).head().row, 2);
        });
    }

    #[gpui::test]
    async fn test_ctrl_alt_click_without_implementations_shows_toast(cx: &mut TestAppContext) {
        let (mut cx, requests) = implementation_click(cx, OpenResultsIn::Picker, &[]).await;

        assert_only_implementation_requests(&requests);
        assert!(active_picker(&mut cx).is_none());
        cx.update_workspace(|workspace, _, _| {
            assert_eq!(workspace.notification_ids().len(), 1);
        });
    }

    #[gpui::test]
    async fn test_ctrl_alt_click_without_implementations_opens_no_tab(cx: &mut TestAppContext) {
        let (mut cx, requests) = implementation_click(cx, OpenResultsIn::MultiBuffer, &[]).await;

        assert_only_implementation_requests(&requests);
        assert!(active_picker(&mut cx).is_none());
        cx.update_workspace(|workspace, _, cx| {
            assert_eq!(workspace.items(cx).count(), 1);
        });
    }

    fn assert_only_implementation_requests(requests: &Mutex<Vec<&'static str>>) {
        let requests = requests.lock().expect("requests").clone();
        assert!(
            !requests.is_empty() && requests.iter().all(|method| *method == "implementation"),
            "no implementation must not fall back to the usages: {requests:?}"
        );
    }
}
