use anyhow::{Context as _, Result};
use async_trait::async_trait;
use futures::StreamExt;
use gpui::AsyncApp;
use http_client::github::latest_github_release;
pub use language::*;
use language::{
    LanguageName, LspAdapterDelegate, LspInstaller, language_settings::LanguageSettings,
};
use lsp::{LanguageServerBinary, LanguageServerName};

use project::lsp_store::language_server_settings;
use regex::Regex;
use serde_json::{Value, json};
use settings::SemanticTokenRules;
use smol::fs;
use std::{
    ffi::{OsStr, OsString},
    future::Future,
    ops::Range,
    path::{Path, PathBuf},
    process::Output,
    str,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering::SeqCst},
    },
};
use util::{ResultExt, fs::remove_matching, maybe, merge_json_value_into};

pub(crate) fn semantic_token_rules() -> SemanticTokenRules {
    let content = grammars::get_file("go/semantic_token_rules.json")
        .expect("missing go/semantic_token_rules.json");
    let json = std::str::from_utf8(&content.data).expect("invalid utf-8 in semantic_token_rules");
    settings::parse_json_with_comments::<SemanticTokenRules>(json)
        .expect("failed to parse go semantic_token_rules.json")
}

fn server_binary_arguments() -> Vec<OsString> {
    vec!["-mode=stdio".into()]
}

#[derive(Copy, Clone)]
pub struct GoLspAdapter;

impl GoLspAdapter {
    const SERVER_NAME: LanguageServerName = LanguageServerName::new_static("gopls");
}

static VERSION_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d+\.\d+\.\d+").expect("Failed to create VERSION_REGEX"));

const BINARY: &str = "gopls";

impl LspInstaller for GoLspAdapter {
    type BinaryVersion = Option<String>;

    async fn fetch_latest_server_version(
        &self,
        delegate: &Arc<dyn LspAdapterDelegate>,
        _: bool,
        cx: &mut AsyncApp,
    ) -> Result<Option<String>> {
        static DID_SHOW_NOTIFICATION: AtomicBool = AtomicBool::new(false);

        const NOTIFICATION_MESSAGE: &str =
            "Could not install the Go language server `gopls`, because `go` was not found.";

        if delegate.which("go".as_ref()).await.is_none() {
            if DID_SHOW_NOTIFICATION
                .compare_exchange(false, true, SeqCst, SeqCst)
                .is_ok()
            {
                cx.update(|cx| {
                    delegate.show_notification(NOTIFICATION_MESSAGE, cx);
                });
            }
            anyhow::bail!(
                "Could not install the Go language server `gopls`, because `go` was not found."
            );
        }

        let release =
            latest_github_release("golang/tools", false, false, delegate.http_client()).await?;
        let version: Option<String> = release.tag_name.strip_prefix("gopls/v").map(str::to_string);
        if version.is_none() {
            log::warn!(
                "couldn't infer gopls version from GitHub release tag name '{}'",
                release.tag_name
            );
        }
        Ok(version)
    }

    async fn check_if_user_installed(
        &self,
        delegate: &Arc<dyn LspAdapterDelegate>,
        _: Option<Toolchain>,
        _: &AsyncApp,
    ) -> Option<LanguageServerBinary> {
        let path = delegate.which(Self::SERVER_NAME.as_ref()).await?;
        Some(LanguageServerBinary {
            path,
            arguments: server_binary_arguments(),
            env: None,
        })
    }

    fn fetch_server_binary(
        &self,
        version: Option<String>,
        container_dir: PathBuf,
        delegate: &Arc<dyn LspAdapterDelegate>,
    ) -> impl Send + Future<Output = Result<LanguageServerBinary>> + use<> {
        let delegate = delegate.clone();

        async move {
            let go = delegate.which("go".as_ref()).await.unwrap_or("go".into());
            let go_version_output = util::command::new_command(&go)
                .args(["version"])
                .output()
                .await
                .context("failed to get go version via `go version` command`")?;
            let go_version = parse_version_output(&go_version_output)?;

            if let Some(version) = version {
                let binary_path = container_dir.join(format!("gopls_{version}_go_{go_version}"));
                if let Ok(metadata) = fs::metadata(&binary_path).await
                    && metadata.is_file()
                {
                    remove_matching(&container_dir, |entry| {
                        entry != binary_path && entry.file_name() != Some(OsStr::new("gobin"))
                    })
                    .await;

                    return Ok(LanguageServerBinary {
                        path: binary_path.to_path_buf(),
                        arguments: server_binary_arguments(),
                        env: None,
                    });
                }
            } else if let Some(path) = get_cached_server_binary(&container_dir).await {
                return Ok(path);
            }

            let gobin_dir = container_dir.join("gobin");
            fs::create_dir_all(&gobin_dir).await?;
            let install_output = util::command::new_command(go)
                .env("GO111MODULE", "on")
                .env("GOBIN", &gobin_dir)
                .args(["install", "golang.org/x/tools/gopls@latest"])
                .output()
                .await?;

            if !install_output.status.success() {
                log::error!(
                    "failed to install gopls via `go install`. stdout: {:?}, stderr: {:?}",
                    String::from_utf8_lossy(&install_output.stdout),
                    String::from_utf8_lossy(&install_output.stderr)
                );
                anyhow::bail!(
                    "failed to install gopls with `go install`. Is `go` installed and in the PATH? Check logs for more information."
                );
            }

            let installed_binary_path = gobin_dir.join(BINARY);
            let version_output = util::command::new_command(&installed_binary_path)
                .arg("version")
                .output()
                .await
                .context("failed to run installed gopls binary")?;
            let gopls_version = parse_version_output(&version_output)?;
            let binary_path = container_dir.join(format!("gopls_{gopls_version}_go_{go_version}"));
            fs::rename(&installed_binary_path, &binary_path).await?;

            Ok(LanguageServerBinary {
                path: binary_path.to_path_buf(),
                arguments: server_binary_arguments(),
                env: None,
            })
        }
    }

    async fn cached_server_binary(
        &self,
        container_dir: PathBuf,
        _: &dyn LspAdapterDelegate,
    ) -> Option<LanguageServerBinary> {
        get_cached_server_binary(&container_dir).await
    }
}

#[async_trait(?Send)]
impl LspAdapter for GoLspAdapter {
    fn name(&self) -> LanguageServerName {
        Self::SERVER_NAME
    }

    async fn initialization_options(
        self: Arc<Self>,
        delegate: &Arc<dyn LspAdapterDelegate>,
        cx: &mut AsyncApp,
    ) -> Result<Option<serde_json::Value>> {
        let semantic_tokens_enabled = cx.update(|cx| {
            LanguageSettings::resolve(None, Some(&LanguageName::new("Go")), cx)
                .semantic_tokens
                .enabled()
        });

        let mut default_config = json!({
            "usePlaceholders": false,
            "hints": {
                "assignVariableTypes": true,
                "compositeLiteralFields": true,
                "compositeLiteralTypes": true,
                "constantValues": true,
                "functionTypeParameters": true,
                "parameterNames": true,
                "rangeVariableTypes": true
            },
            "semanticTokens": semantic_tokens_enabled
        });

        let project_initialization_options = cx.update(|cx| {
            language_server_settings(delegate.as_ref(), &self.name(), cx)
                .and_then(|s| s.initialization_options.clone())
        });

        if let Some(override_options) = project_initialization_options {
            merge_json_value_into(override_options, &mut default_config);
        }

        Ok(Some(default_config))
    }

    async fn workspace_configuration(
        self: Arc<Self>,
        delegate: &Arc<dyn LspAdapterDelegate>,
        _: Option<Toolchain>,
        _: Option<lsp::Uri>,
        cx: &mut AsyncApp,
    ) -> Result<Value> {
        Ok(cx
            .update(|cx| {
                language_server_settings(delegate.as_ref(), &self.name(), cx)
                    .and_then(|settings| settings.settings.clone())
            })
            .unwrap_or_default())
    }

    async fn label_for_completion(
        &self,
        completion: &lsp::CompletionItem,
        language: &Arc<Language>,
    ) -> Option<CodeLabel> {
        let label = &completion.label;

        // Gopls returns nested fields and methods as completions.
        // To syntax highlight these, combine their final component
        // with their detail.
        let name_offset = label.rfind('.').unwrap_or(0);

        match completion.kind.zip(completion.detail.as_ref()) {
            Some((lsp::CompletionItemKind::MODULE, detail)) => {
                let text = format!("{label} {detail}");
                let source = format!("import {text}");
                let runs = language
                    .highlight_text(source.as_str(), 7..7 + text[name_offset..].len())
                    .collect();
                let filter_range = completion
                    .filter_text
                    .as_deref()
                    .and_then(|filter_text| {
                        text.find(filter_text)
                            .map(|start| start..start + filter_text.len())
                    })
                    .unwrap_or(0..label.len());
                return Some(CodeLabel::new(text, filter_range, runs));
            }
            Some((
                lsp::CompletionItemKind::CONSTANT | lsp::CompletionItemKind::VARIABLE,
                detail,
            )) => {
                let text = format!("{label} {detail}");
                let source = format!("var {} {}", &text[name_offset..], detail);
                let runs = adjust_runs(
                    name_offset,
                    language.highlight_text(source.as_str(), 4..4 + text[name_offset..].len()),
                );
                let filter_range = completion
                    .filter_text
                    .as_deref()
                    .and_then(|filter_text| {
                        text.find(filter_text)
                            .map(|start| start..start + filter_text.len())
                    })
                    .unwrap_or(0..label.len());
                return Some(CodeLabel::new(text, filter_range, runs));
            }
            Some((lsp::CompletionItemKind::STRUCT, _)) => {
                let text = format!("{label} struct {{}}");
                let source = format!("type {}", &text[name_offset..]);
                let runs = adjust_runs(
                    name_offset,
                    language.highlight_text(source.as_str(), 5..5 + text[name_offset..].len()),
                );
                let filter_range = completion
                    .filter_text
                    .as_deref()
                    .and_then(|filter_text| {
                        text.find(filter_text)
                            .map(|start| start..start + filter_text.len())
                    })
                    .unwrap_or(0..label.len());
                return Some(CodeLabel::new(text, filter_range, runs));
            }
            Some((lsp::CompletionItemKind::INTERFACE, _)) => {
                let text = format!("{label} interface {{}}");
                let source = format!("type {}", &text[name_offset..]);
                let runs = adjust_runs(
                    name_offset,
                    language.highlight_text(source.as_str(), 5..5 + text[name_offset..].len()),
                );
                let filter_range = completion
                    .filter_text
                    .as_deref()
                    .and_then(|filter_text| {
                        text.find(filter_text)
                            .map(|start| start..start + filter_text.len())
                    })
                    .unwrap_or(0..label.len());
                return Some(CodeLabel::new(text, filter_range, runs));
            }
            Some((lsp::CompletionItemKind::FIELD, detail)) => {
                let text = format!("{label} {detail}");
                let source = format!("type T struct {{ {} }}", &text[name_offset..]);
                let runs = adjust_runs(
                    name_offset,
                    language.highlight_text(source.as_str(), 16..16 + text[name_offset..].len()),
                );
                let filter_range = completion
                    .filter_text
                    .as_deref()
                    .and_then(|filter_text| {
                        text.find(filter_text)
                            .map(|start| start..start + filter_text.len())
                    })
                    .unwrap_or(0..label.len());
                return Some(CodeLabel::new(text, filter_range, runs));
            }
            Some((lsp::CompletionItemKind::FUNCTION | lsp::CompletionItemKind::METHOD, detail)) => {
                if let Some(signature) = detail.strip_prefix("func") {
                    let text = format!("{label}{signature}");
                    let source = format!("func {} {{}}", &text[name_offset..]);
                    let runs = adjust_runs(
                        name_offset,
                        language.highlight_text(source.as_str(), 5..5 + text[name_offset..].len()),
                    );
                    let filter_range = completion
                        .filter_text
                        .as_deref()
                        .and_then(|filter_text| {
                            text.find(filter_text)
                                .map(|start| start..start + filter_text.len())
                        })
                        .unwrap_or(0..label.len());
                    return Some(CodeLabel::new(text, filter_range, runs));
                }
            }
            _ => {}
        }
        None
    }

    async fn label_for_symbol(
        &self,
        symbol: &language::Symbol,
        language: &Arc<Language>,
    ) -> Option<CodeLabel> {
        let name = &symbol.name;
        let (text, filter_range, display_range) = match symbol.kind {
            language::SymbolKind::Method | language::SymbolKind::Function => {
                let text = format!("func {} () {{}}", name);
                let filter_range = 5..5 + name.len();
                let display_range = 0..filter_range.end;
                (text, filter_range, display_range)
            }
            language::SymbolKind::Struct => {
                let text = format!("type {} struct {{}}", name);
                let filter_range = 5..5 + name.len();
                let display_range = 0..text.len();
                (text, filter_range, display_range)
            }
            language::SymbolKind::Interface => {
                let text = format!("type {} interface {{}}", name);
                let filter_range = 5..5 + name.len();
                let display_range = 0..text.len();
                (text, filter_range, display_range)
            }
            language::SymbolKind::Class => {
                let text = format!("type {} T", name);
                let filter_range = 5..5 + name.len();
                let display_range = 0..filter_range.end;
                (text, filter_range, display_range)
            }
            language::SymbolKind::Constant => {
                let text = format!("const {} = nil", name);
                let filter_range = 6..6 + name.len();
                let display_range = 0..filter_range.end;
                (text, filter_range, display_range)
            }
            language::SymbolKind::Variable => {
                let text = format!("var {} = nil", name);
                let filter_range = 4..4 + name.len();
                let display_range = 0..filter_range.end;
                (text, filter_range, display_range)
            }
            language::SymbolKind::Module => {
                let text = format!("package {}", name);
                let filter_range = 8..8 + name.len();
                let display_range = 0..filter_range.end;
                (text, filter_range, display_range)
            }
            _ => return None,
        };

        Some(CodeLabel::new(
            text[display_range.clone()].to_string(),
            filter_range,
            language
                .highlight_text(text.as_str(), display_range)
                .collect(),
        ))
    }

    fn diagnostic_message_to_markdown(&self, message: &str) -> Option<String> {
        static REGEX: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?m)\n\s*").expect("Failed to create REGEX"));
        Some(REGEX.replace_all(message, "\n\n").to_string())
    }
}

fn parse_version_output(output: &Output) -> Result<&str> {
    let version_stdout =
        str::from_utf8(&output.stdout).context("version command produced invalid utf8 output")?;

    let version = VERSION_REGEX
        .find(version_stdout)
        .with_context(|| format!("failed to parse version output '{version_stdout}'"))?
        .as_str();

    Ok(version)
}

async fn get_cached_server_binary(container_dir: &Path) -> Option<LanguageServerBinary> {
    maybe!(async {
        let mut last_binary_path = None;
        let mut entries = fs::read_dir(container_dir).await?;
        while let Some(entry) = entries.next().await {
            let entry = entry?;
            if entry.file_type().await?.is_file()
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("gopls_"))
            {
                last_binary_path = Some(entry.path());
            }
        }

        let path = last_binary_path.context("no cached binary")?;
        anyhow::Ok(LanguageServerBinary {
            path,
            arguments: server_binary_arguments(),
            env: None,
        })
    })
    .await
    .log_err()
}

fn adjust_runs(
    delta: usize,
    runs: impl Iterator<Item = (Range<usize>, HighlightId)>,
) -> Vec<(Range<usize>, HighlightId)> {
    runs.map(|(range, highlight_id)| (range.start + delta..range.end + delta, highlight_id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language;
    use gpui::{AppContext, Hsla, TestAppContext};
    use theme::SyntaxTheme;
    use unindent::Unindent as _;

    fn go_language() -> Arc<Language> {
        language("go", tree_sitter_go::LANGUAGE.into())
    }

    #[gpui::test]
    async fn test_go_label_for_completion() {
        let adapter = Arc::new(GoLspAdapter);
        let language = go_language();

        let theme = SyntaxTheme::new_test([
            ("type", Hsla::default()),
            ("keyword", Hsla::default()),
            ("function", Hsla::default()),
            ("number", Hsla::default()),
            ("property", Hsla::default()),
        ]);
        language.set_theme(&theme);

        let grammar = language.grammar().unwrap();
        let highlight_function = grammar.highlight_id_for_name("function").unwrap();
        let highlight_type = grammar.highlight_id_for_name("type").unwrap();
        let highlight_keyword = grammar.highlight_id_for_name("keyword").unwrap();
        let highlight_number = grammar.highlight_id_for_name("number").unwrap();
        let highlight_field = grammar.highlight_id_for_name("property").unwrap();

        assert_eq!(
            adapter
                .label_for_completion(
                    &lsp::CompletionItem {
                        kind: Some(lsp::CompletionItemKind::FUNCTION),
                        label: "Hello".to_string(),
                        detail: Some("func(a B) c.D".to_string()),
                        ..Default::default()
                    },
                    &language
                )
                .await,
            Some(CodeLabel::new(
                "Hello(a B) c.D".to_string(),
                0..5,
                vec![
                    (0..5, highlight_function),
                    (8..9, highlight_type),
                    (13..14, highlight_type),
                ]
            ))
        );

        // Nested methods
        assert_eq!(
            adapter
                .label_for_completion(
                    &lsp::CompletionItem {
                        kind: Some(lsp::CompletionItemKind::METHOD),
                        label: "one.two.Three".to_string(),
                        detail: Some("func() [3]interface{}".to_string()),
                        ..Default::default()
                    },
                    &language
                )
                .await,
            Some(CodeLabel::new(
                "one.two.Three() [3]interface{}".to_string(),
                0..13,
                vec![
                    (8..13, highlight_function),
                    (17..18, highlight_number),
                    (19..28, highlight_keyword),
                ],
            ))
        );

        // Nested fields
        assert_eq!(
            adapter
                .label_for_completion(
                    &lsp::CompletionItem {
                        kind: Some(lsp::CompletionItemKind::FIELD),
                        label: "two.Three".to_string(),
                        detail: Some("a.Bcd".to_string()),
                        ..Default::default()
                    },
                    &language
                )
                .await,
            Some(CodeLabel::new(
                "two.Three a.Bcd".to_string(),
                0..9,
                vec![(4..9, highlight_field), (12..15, highlight_type)],
            ))
        );
    }

    #[gpui::test]
    fn test_go_outline_includes_methods_with_receiver_forms(cx: &mut TestAppContext) {
        let language = go_language();

        let source = r#"
        package main

        type v2 struct{}

        func (v2) BrokenMethod() {
            println("start")
        }

        func (_ v2) UnderscoreReceiverMethod() {
            println("start")
        }

        func (v v2) NamedReceiverMethod() {
            println("start")
        }

        func (v *v2) PointerReceiverMethod() {
            println("start")
        }

        func WorkingFunction() {
            println("start")
        }
        "#
        .unindent();

        let buffer =
            cx.new(|cx| crate::Buffer::local(source.clone(), cx).with_language(language, cx));
        let snapshot = buffer.read_with(cx, |buffer, _| buffer.snapshot());
        let outline = snapshot.outline(None);

        assert_eq!(
            outline
                .items
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>(),
            &[
                "type v2",
                "func (v2) BrokenMethod",
                "func (_ v2) UnderscoreReceiverMethod",
                "func (v v2) NamedReceiverMethod",
                "func (v *v2) PointerReceiverMethod",
                "func WorkingFunction",
            ]
        );

        for (method_name, expected_symbol) in [
            ("BrokenMethod", "func (v2) BrokenMethod"),
            (
                "UnderscoreReceiverMethod",
                "func (_ v2) UnderscoreReceiverMethod",
            ),
            ("NamedReceiverMethod", "func (v v2) NamedReceiverMethod"),
            (
                "PointerReceiverMethod",
                "func (v *v2) PointerReceiverMethod",
            ),
            ("WorkingFunction", "func WorkingFunction"),
        ] {
            let method_position = source
                .find(&format!("{method_name}()"))
                .expect("method should exist in source");
            let body_position = source[method_position..]
                .find("println")
                .map(|body_offset| method_position + body_offset)
                .expect("method should contain a body");
            let symbols = snapshot.symbols_containing(body_position, None);
            assert_eq!(
                symbols
                    .last()
                    .map(|item| item.text.as_str())
                    .expect("method should have an outline symbol"),
                expected_symbol
            );
        }
    }
}
