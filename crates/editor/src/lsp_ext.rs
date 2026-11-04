use std::sync::Arc;

use crate::Editor;
use collections::HashSet;
use gpui::{App, Entity};
use language::Buffer;
use language::Language;
use lsp::LanguageServerId;
use lsp::LanguageServerName;

pub(crate) fn find_specific_language_server_in_selection<F>(
    editor: &Editor,
    cx: &mut App,
    filter_language: F,
    language_server_name: LanguageServerName,
) -> Option<(
    text::Anchor,
    Arc<Language>,
    LanguageServerId,
    Entity<Buffer>,
)>
where
    F: Fn(&Language) -> bool,
{
    let project = editor.project.clone()?;
    let multi_buffer = editor.buffer();
    let mut seen_buffer_ids = HashSet::default();
    editor
        .selections
        .disjoint_anchors_arc()
        .iter()
        .find_map(|selection| {
            let multi_buffer = multi_buffer.read(cx);
            let multi_buffer_snapshot = multi_buffer.snapshot(cx);
            let (position, buffer) = multi_buffer_snapshot
                .anchor_to_buffer_anchor(selection.head())
                .and_then(|(anchor, _)| Some((anchor, multi_buffer.buffer(anchor.buffer_id)?)))?;
            if !seen_buffer_ids.insert(buffer.read(cx).remote_id()) {
                return None;
            }

            let language = buffer.read(cx).language_at(position)?;
            if filter_language(&language) {
                let server_id = buffer.update(cx, |buffer, cx| {
                    project
                        .read(cx)
                        .language_server_id_for_name(buffer, &language_server_name, cx)
                })?;
                Some((position, language, server_id, buffer))
            } else {
                None
            }
        })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures::StreamExt as _;
    use gpui::{AppContext as _, Entity, TestAppContext};
    use language::{FakeLspAdapter, Language};
    use languages::rust_lang;
    use lsp::{LanguageServerId, LanguageServerName};
    use multi_buffer::MultiBuffer;
    use project::{FakeFs, Project};
    use util::path;

    use crate::{MoveToEnd, editor_tests::init_test, test::build_editor_with_project};

    use super::find_specific_language_server_in_selection;

    #[gpui::test]
    async fn test_find_language_server_at_end_of_file(cx: &mut TestAppContext) {
        init_test(cx, |_| {});

        let fs = FakeFs::new(cx.executor());
        fs.insert_file(path!("/file.rs"), "fn main() {}".into())
            .await;

        let project = Project::test(fs, [path!("/file.rs").as_ref()], cx).await;
        let language_registry = project.read_with(cx, |project, _| project.languages().clone());
        language_registry.add(rust_lang());
        let mut fake_servers =
            language_registry.register_fake_lsp("Rust", FakeLspAdapter::default());

        let underlying_buffer = project
            .update(cx, |project, cx| {
                project.open_local_buffer(path!("/file.rs"), cx)
            })
            .await
            .unwrap();

        let buffer = cx.new(|cx| MultiBuffer::singleton(underlying_buffer.clone(), cx));
        let (editor, cx) = cx.add_window_view(|window, cx| {
            build_editor_with_project(project.clone(), buffer, window, cx)
        });

        let fake_server = fake_servers.next().await.unwrap();
        cx.executor().run_until_parked();

        let expected_server_id = fake_server.server.server_id();
        let language_server_name = LanguageServerName::new_static("the-fake-language-server");
        let filter = |language: &Language| language.name().as_ref() == "Rust";

        let assert_result = |result: Option<(
            text::Anchor,
            Arc<Language>,
            LanguageServerId,
            Entity<language::Buffer>,
        )>,
                             message: &str| {
            let (_, language, server_id, buffer) = result.expect(message);
            assert_eq!(
                language.name().as_ref(),
                "Rust",
                "{message}: wrong language"
            );
            assert_eq!(server_id, expected_server_id, "{message}: wrong server ID");
            assert_eq!(buffer, underlying_buffer, "{message}: wrong buffer");
        };

        editor.update(cx, |editor, cx| {
            assert_result(
                find_specific_language_server_in_selection(
                    editor,
                    cx,
                    filter,
                    language_server_name.clone(),
                ),
                "should find correct language server at beginning of file",
            );
        });

        editor.update_in(cx, |editor, window, cx| {
            editor.move_to_end(&MoveToEnd, window, cx);
        });

        editor.update(cx, |editor, cx| {
            assert_result(
                find_specific_language_server_in_selection(
                    editor,
                    cx,
                    filter,
                    language_server_name.clone(),
                ),
                "should find correct language server at end of file",
            );
        });
    }
}
