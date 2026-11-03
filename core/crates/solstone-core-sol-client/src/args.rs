// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[derive(Debug, Default)]
pub(crate) struct ParsedArgs {
    pub(crate) values: Vec<(String, String)>,
    pub(crate) flags: Vec<String>,
    pub(crate) positionals: Vec<String>,
}

impl ParsedArgs {
    pub(crate) fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(key, _value)| key == name)
            .map(|(_key, value)| value.as_str())
    }

    pub(crate) fn has_flag(&self, name: &str) -> bool {
        self.flags.iter().any(|flag| flag == name)
    }
}

pub(crate) fn parse_args(
    args: &[String],
    options: &[&str],
    flags: &[&str],
) -> Result<ParsedArgs, String> {
    let mut parsed = ParsedArgs::default();
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if let Some((name, value)) = token.split_once('=')
            && options.contains(&name)
        {
            parsed.values.push((name.to_string(), value.to_string()));
        } else if options.contains(&token.as_str()) {
            index += 1;
            let Some(value) = args.get(index) else {
                return Err(format!("Error: option {token} requires an argument."));
            };
            parsed.values.push((token.clone(), value.clone()));
        } else if flags.contains(&token.as_str()) {
            parsed.flags.push(token.clone());
        } else if token.starts_with('-') {
            return Err(format!("Error: unknown option {token}."));
        } else {
            parsed.positionals.push(token.clone());
        }
        index += 1;
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::handler_for;
    use crate::command::{CommandContext, CommandOutput};
    use crate::seam::{ExpectedHttpCall, ScriptedHttpTransport};
    use crate::transport::{ApiRequest, HttpMethod, HttpResponse, QueryParam, TimeoutPolicy};
    use serde_json::{Value, json};
    use std::collections::BTreeMap;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn preserves_occurrence_order_spelling_and_uninterpreted_values() {
        let parsed = parse_args(
            &argv(&[
                "first",
                "--name=",
                "-n",
                "--flag",
                "--flag",
                "--name=a=b",
                "last",
            ]),
            &["--name", "-n"],
            &["--flag"],
        )
        .unwrap();
        assert_eq!(
            parsed.values,
            vec![
                ("--name".to_owned(), String::new()),
                ("-n".to_owned(), "--flag".to_owned()),
                ("--name".to_owned(), "a=b".to_owned()),
            ]
        );
        assert_eq!(parsed.value("--name"), Some("a=b"));
        assert_eq!(parsed.value("-n"), Some("--flag"));
        assert_eq!(parsed.flags, argv(&["--flag"]));
        assert_eq!(parsed.positionals, argv(&["first", "last"]));
    }

    #[test]
    fn rejects_the_first_unknown_or_missing_option() {
        for (args, error) in [
            (vec!["--", "--name"], "Error: unknown option --."),
            (vec!["--flag=true"], "Error: unknown option --flag=true."),
            (vec!["--unknown=x"], "Error: unknown option --unknown=x."),
            (
                vec!["positional", "--bad", "--name"],
                "Error: unknown option --bad.",
            ),
            (vec!["--name"], "Error: option --name requires an argument."),
            (vec!["-n"], "Error: option -n requires an argument."),
        ] {
            assert_eq!(
                parse_args(&argv(&args), &["--name", "-n"], &["--flag"]).unwrap_err(),
                error
            );
        }
    }

    fn ok_get(path: &str, body: Value) -> ExpectedHttpCall {
        ExpectedHttpCall::Request {
            expected: ApiRequest {
                method: HttpMethod::Get,
                path: path.to_owned(),
                params: vec![],
                json: None,
                headers: vec![],
                policy: TimeoutPolicy::Api,
            },
            result: Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: body.to_string().into_bytes(),
                policy: TimeoutPolicy::Api,
            }),
        }
    }

    #[test]
    fn link_status_without_an_instance_id_tells_no_identity_from_a_failed_read() {
        let no_identity_yet = crate::mark_text::NO_IDENTITY_YET_LINE;
        let unavailable = crate::mark_text::MARK_UNAVAILABLE_LINE;
        for (availability, shown, not_shown) in [
            ("none", no_identity_yet, unavailable),
            ("unavailable", unavailable, no_identity_yet),
        ] {
            let transport = ScriptedHttpTransport::new(vec![
                ok_get("/app/network/api/status", json!({"instance_id": null})),
                ok_get("/app/network/api/private-link", json!({})),
                ok_get("/app/network/api/devices", json!({"devices": []})),
                ok_get(
                    "/app/network/api/identity",
                    json!({"availability": availability}),
                ),
            ]);
            let output = run(&["link", "status"], &[], &transport);
            transport.assert_done();
            assert_eq!(output.exit, 0, "{availability}");
            assert!(output.stdout.contains(shown), "{availability}");
            assert!(!output.stdout.contains(not_shown), "{availability}");
        }
    }

    fn run(path: &[&str], args: &[&str], transport: &ScriptedHttpTransport) -> CommandOutput {
        let (_, handler) = handler_for(path).expect("registered command");
        handler(CommandContext {
            args: &argv(args),
            env: &BTreeMap::new(),
            stdin: "",
            transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
            link_status_probe: None,
        })
    }

    struct Case<'a> {
        command: &'a [&'a str],
        args: &'a [&'a str],
        method: HttpMethod,
        route: &'a str,
        params: Vec<QueryParam>,
        body: Option<Value>,
        response: Value,
        stdout: &'a str,
    }

    #[test]
    fn migrated_commands_preserve_requests_output_and_exit() {
        let cases = [
            Case {
                command: &["body", "window"],
                args: &[
                    "--from=old",
                    "--from=",
                    "--to",
                    "--json",
                    "--json",
                    "--json",
                ],
                method: HttpMethod::Get,
                route: "/app/body/api/window",
                params: vec![
                    QueryParam::single("from", ""),
                    QueryParam::single("to", "--json"),
                ],
                body: None,
                response: json!({}),
                stdout: "{}\n",
            },
            Case {
                command: &["transcripts", "read"],
                args: &[
                    "20261012",
                    "--segment=old",
                    "--segment",
                    "--audio",
                    "--stream=",
                    "--audio",
                ],
                method: HttpMethod::Get,
                route: "/app/transcripts/api/read/20261012",
                params: vec![
                    QueryParam::single("transcripts", "1"),
                    QueryParam::single("percepts", "0"),
                    QueryParam::single("agents", "0"),
                    QueryParam::single("segment", "--audio"),
                    QueryParam::single("stream", ""),
                ],
                body: None,
                response: json!({"markdown": "sample"}),
                stdout: "sample\n",
            },
            Case {
                command: &["journal", "imports"],
                args: &[
                    "-n",
                    "1",
                    "--limit=2",
                    "-n=3",
                    "--source=old",
                    "-s=",
                    "--json",
                ],
                method: HttpMethod::Get,
                route: "/app/import/api/list",
                params: vec![
                    QueryParam::single("per_page", "3"),
                    QueryParam::single("source", ""),
                ],
                body: None,
                response: json!({}),
                stdout: "{}\n",
            },
            Case {
                command: &["link", "local-network", "status"],
                args: &["unused", "--json", "--json"],
                method: HttpMethod::Get,
                route: "/app/network/api/local-network",
                params: vec![],
                body: None,
                response: json!({}),
                stdout: "{}\n",
            },
            Case {
                command: &["thinking", "set-local-endpoint"],
                args: &[
                    "--url=old",
                    "--url=",
                    "--model",
                    "--credential",
                    "--credential=",
                ],
                method: HttpMethod::Post,
                route: "/app/thinking/api/local/endpoint",
                params: vec![],
                body: Some(
                    json!({"endpoint_url": "", "served_model_id": "--credential", "credential": ""}),
                ),
                response: json!({}),
                stdout: "{}\n",
            },
            Case {
                command: &["profile", "full"],
                args: &[
                    "sample",
                    "--facets=old",
                    "--facets=",
                    "--include-mentions",
                    "--include-mentions",
                    "--json",
                ],
                method: HttpMethod::Get,
                route: "/api/profile/sample",
                params: vec![
                    QueryParam::single("facets", ""),
                    QueryParam::single("include_mentions", "true"),
                ],
                body: None,
                response: json!({}),
                stdout: "{}\n",
            },
        ];
        for case in cases {
            let transport = ScriptedHttpTransport::new(vec![ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: case.method,
                    path: case.route.to_owned(),
                    params: case.params,
                    json: case.body,
                    headers: vec![],
                    policy: TimeoutPolicy::Api,
                },
                result: Ok(HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: case.response.to_string().into_bytes(),
                    policy: TimeoutPolicy::Api,
                }),
            }]);
            assert_eq!(
                run(case.command, case.args, &transport),
                CommandOutput::success(case.stdout),
                "{:?}",
                case.command
            );
            transport.assert_done();
        }
    }

    #[test]
    fn migrated_commands_keep_scan_errors_before_command_validation() {
        for (command, option, exit) in [
            (vec!["body", "window"], "--from", 1),
            (vec!["transcripts", "read"], "--max", 1),
            (vec!["journal", "imports"], "-n", 1),
            (vec!["link", "pair"], "--as", 1),
            (vec!["thinking", "set-local-endpoint"], "--url", 1),
            (vec!["profile", "full"], "--facets", 1),
        ] {
            for (args, error) in [
                (
                    vec!["positional", "--", option],
                    "Error: unknown option --.".to_owned(),
                ),
                (
                    vec![option],
                    format!("Error: option {option} requires an argument."),
                ),
                (
                    vec![option, "invalid", "--bad"],
                    "Error: unknown option --bad.".to_owned(),
                ),
            ] {
                let transport = ScriptedHttpTransport::new(vec![]);
                assert_eq!(
                    run(&command, &args, &transport),
                    CommandOutput::failure(format!("{error}\n"), exit),
                    "{command:?} {args:?}"
                );
                transport.assert_done();
            }
        }
        let transport = ScriptedHttpTransport::new(vec![]);
        assert_eq!(
            run(&["link", "local-network", "status"], &["--"], &transport),
            CommandOutput::failure("Error: unknown option --.\n", 2)
        );
        transport.assert_done();
    }
}
