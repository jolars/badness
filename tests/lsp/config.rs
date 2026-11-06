//! Exercise config fallbacks in child processes so their environments stay
//! isolated from the in-process LSP tests.

use std::io::BufReader;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use badness::formatter::{FormatStyle, WrapMode, format_with_style};
use crossbeam_channel::{Receiver, unbounded};
use lsp_server::{Message, Notification, Request};
use serde_json::{Value, json};

use super::{apply_edits, path_to_file_uri};

struct Server {
    child: Child,
    messages: Receiver<Message>,
    reader: Option<std::thread::JoinHandle<()>>,
    next_id: i32,
}

impl Server {
    fn start(project: &Path, config: &Path, global: bool) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_badness"));
        command
            .arg("lsp")
            .current_dir(project)
            .env_remove("BADNESS_CONFIG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if global {
            command.env(
                "XDG_CONFIG_HOME",
                config.parent().unwrap().parent().unwrap(),
            );
        } else {
            command.env("BADNESS_CONFIG", config);
        }
        let mut child = command.spawn().expect("start language server");
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let (sender, messages) = unbounded();
        let reader = std::thread::spawn(move || {
            while let Some(message) = Message::read(&mut stdout).expect("read server message") {
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        let mut server = Self {
            child,
            messages,
            reader: Some(reader),
            next_id: 0,
        };
        server.request(
            "initialize",
            json!({
                "capabilities": {},
                "initializationOptions": {
                    "lineWidth": 40,
                    "indentWidth": 6,
                    "texmf": { "enabled": false }
                }
            }),
        );
        server.notify("initialized", json!({}));
        server
    }

    fn send(&mut self, message: Message) {
        message
            .write(self.child.stdin.as_mut().unwrap())
            .expect("write client message");
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(Message::Notification(Notification::new(
            method.to_owned(),
            params,
        )));
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id.into();
        self.send(Message::Request(Request::new(
            id,
            method.to_owned(),
            params,
        )));
        loop {
            match self
                .messages
                .recv_timeout(Duration::from_secs(10))
                .expect("server response")
            {
                Message::Response(response) => {
                    assert_eq!(response.id, self.next_id.into());
                    return response.response_result.expect("successful response");
                }
                Message::Notification(_) => {}
                Message::Request(request) => panic!("unexpected server request: {request:?}"),
            }
        }
    }

    fn shutdown(mut self) {
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        assert!(self.child.wait().unwrap().success());
        self.reader.take().unwrap().join().unwrap();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // A failed assertion must not leave a language server running.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn formatting_widths_respect_global_key_presence_and_project_precedence() {
    const ENVIRONMENT: &str = "\\begin{itemize}\n\\item First item.\n\\end{itemize}\n";
    const PARAGRAPH: &str = "\nOne two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen.\n";
    let source = format!("{ENVIRONMENT}{PARAGRAPH}");

    for global in [true, false] {
        for (settings, line_width, indent_width, wrap) in [
            ("", 40, None, WrapMode::Reflow),
            (
                "[lint]\nignore = ['missing-nonbreaking-space']\n",
                40,
                None,
                WrapMode::Reflow,
            ),
            (
                "[format]\nwrap = 'preserve'\n",
                40,
                None,
                WrapMode::Preserve,
            ),
            ("[format]\nline-width = 50\n", 50, None, WrapMode::Reflow),
            ("[format]\nline-width = 80\n", 80, None, WrapMode::Reflow),
            ("[format]\nline-width = 0\n", 0, None, WrapMode::Reflow),
            (
                "[format]\nindent-width = 8\n",
                40,
                Some(8),
                WrapMode::Reflow,
            ),
            (
                "[format]\nindent-width = 2\n",
                40,
                Some(2),
                WrapMode::Reflow,
            ),
            (
                "[format]\nline-width = 80\nindent-width = 2\n",
                80,
                Some(2),
                WrapMode::Reflow,
            ),
            (
                "extend = 'middle.toml'\n[format]\nline-width = 80\n",
                80,
                Some(8),
                WrapMode::Preserve,
            ),
            ("extend = 'empty.toml'\n", 40, None, WrapMode::Reflow),
        ] {
            for project_config in [false, true] {
                let directory = tempfile::tempdir().unwrap();
                let project = directory.path().join("project");
                std::fs::create_dir_all(project.join(".git")).unwrap();
                let config_dir = directory.path().join("xdg/badness");
                std::fs::create_dir_all(&config_dir).unwrap();
                let config = config_dir.join("config.toml");
                std::fs::write(&config, settings).unwrap();
                std::fs::write(config_dir.join("empty.toml"), "[format]\n").unwrap();
                std::fs::write(config_dir.join("middle.toml"), "extend = 'base.toml'\n").unwrap();
                std::fs::write(
                    config_dir.join("base.toml"),
                    "[format]\nindent-width = 8\nline-width = 50\nwrap = 'preserve'\n",
                )
                .unwrap();
                if project_config {
                    std::fs::write(project.join("badness.toml"), "[lint]\nignore = []\n").unwrap();
                }

                let mut server = Server::start(&project, &config, global);
                let uri = path_to_file_uri(&project.join("main.tex"));
                server.notify(
                    "textDocument/didOpen",
                    json!({ "textDocument": {
                        "uri": uri, "languageId": "latex", "version": 1, "text": source
                    }}),
                );
                for tab_size in [4, 0] {
                    let style = if project_config {
                        FormatStyle::default()
                    } else {
                        FormatStyle {
                            line_width,
                            indent_width: indent_width.unwrap_or(if tab_size > 0 { 4 } else { 6 }),
                            wrap,
                            ..FormatStyle::default()
                        }
                    };
                    for method in ["formatting", "rangeFormatting", "onTypeFormatting"] {
                        let mut params = json!({
                            "textDocument": { "uri": uri },
                            "options": { "tabSize": tab_size, "insertSpaces": true }
                        });
                        if method == "rangeFormatting" {
                            params["range"] = json!({
                                "start": { "line": 0, "character": 0 },
                                "end": { "line": 5, "character": 0 }
                            });
                        } else if method == "onTypeFormatting" {
                            params["position"] = json!({ "line": 2, "character": 13 });
                            params["ch"] = json!("}");
                        }
                        let result = server.request(&format!("textDocument/{method}"), params);
                        let edits: Vec<lsp_types::TextEdit> =
                            serde_json::from_value(result).unwrap();
                        let expected = if method == "onTypeFormatting" {
                            format!(
                                "{}{PARAGRAPH}",
                                format_with_style(ENVIRONMENT, style).unwrap()
                            )
                        } else {
                            format_with_style(&source, style).unwrap()
                        };
                        assert_eq!(
                            apply_edits(&source, &edits),
                            expected,
                            "{method}, global={global}, project={project_config}, tabSize={tab_size}, config={settings:?}"
                        );
                    }
                }
                server.shutdown();
            }
        }
    }
}
