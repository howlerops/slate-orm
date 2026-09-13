//! What the binary will not start on.
//!
//! A refusal is a feature here, so each one is tested the way a feature is:
//! the process runs, exits non-zero, and says something that names the setting
//! rather than the code path. The exit code is 2 throughout — distinct from 1
//! so a supervisor can tell "this configuration is wrong" from "it started and
//! then died".
//!
//! The authentication cases are the point of the file. `PROTOCOL-FINDINGS.md`
//! predicts that every hand-written head node "will get the `Authenticator`
//! choice slightly wrong in its own way"; these are the ways, made impossible.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    unreachable_pub
)]

mod harness;

use harness::{Files, run};

/// A configuration that is valid apart from whatever a test breaks.
const GOOD: &str = r#"
[listen]
address = "127.0.0.1:0"

[auth]
mode = "trusted-header"

[storage]
backend = "memory"

[[tables]]
name = "docs"
id = 1
columns = [
  { name = "id",   type = "u64" },
  { name = "kind", type = "str" },
  { name = "size", type = "i64" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["docs"]
actions = ["all"]
"#;

/// `--check` the given configuration and require it to be refused.
///
/// `--check` rather than a real start: the refusals below all happen before
/// anything is opened or bound, and asserting that is part of the point — a
/// configuration this bad should never reach a socket.
#[track_caller]
fn refused(configuration: &str) -> String {
    let files = Files::new();
    let path = files.write("head.toml", configuration);
    let finished = run(&["--config", &path.display().to_string(), "--check"]);
    assert_eq!(
        finished.code,
        Some(2),
        "this configuration should have been refused:\n{}",
        finished.output()
    );
    finished.output()
}

/// `--check` the given configuration and require it to pass.
#[track_caller]
fn accepted(configuration: &str) -> String {
    let files = Files::new();
    let path = files.write("head.toml", configuration);
    let finished = run(&["--config", &path.display().to_string(), "--check"]);
    assert_eq!(
        finished.code,
        Some(0),
        "this configuration should have been accepted:\n{}",
        finished.output()
    );
    finished.output()
}

// ── authentication ──────────────────────────────────────────────────────────

#[test]
fn a_configuration_with_no_auth_section_will_not_start() {
    // The single most important refusal in this crate: there is no default,
    // and the absence of the section is not one.
    let output = refused(&GOOD.replace("[auth]\nmode = \"trusted-header\"\n", ""));
    assert!(output.contains("no `[auth]` section"), "{output}");
    assert!(output.contains("Every default is wrong"), "{output}");
    for mode in ["deny-all", "trusted-header", "token"] {
        assert!(
            output.contains(mode),
            "the modes should be listed: {output}"
        );
    }
}

#[test]
fn trusted_header_off_loopback_must_name_its_proxy() {
    let output = refused(&GOOD.replace("127.0.0.1:0", "0.0.0.0:50051"));
    assert!(
        output.contains("header_source = \"trusted-proxy\""),
        "{output}"
    );
    assert!(output.contains("open door"), "{output}");
}

#[test]
fn the_same_configuration_is_fine_on_loopback() {
    // The contrast is the argument: the rule is about the address, not about
    // the mode being distrusted everywhere.
    accepted(GOOD);
}

#[test]
fn the_listen_override_is_what_the_auth_rules_are_applied_to() {
    // A loopback file plus `--listen 0.0.0.0:…` is a public server, and the
    // check follows the address that will actually be bound rather than the
    // one written down.
    let files = Files::new();
    let path = files.write("head.toml", GOOD);
    let finished = run(&[
        "--config",
        &path.display().to_string(),
        "--listen",
        "0.0.0.0:50051",
        "--check",
    ]);
    assert_eq!(finished.code, Some(2), "{}", finished.output());
    assert!(
        finished.output().contains("trusted-proxy"),
        "{}",
        finished.output()
    );
}

#[test]
fn an_unspecified_ipv6_address_is_not_loopback_either() {
    let output = refused(&GOOD.replace("127.0.0.1:0", "[::]:50051"));
    assert!(output.contains("trusted-proxy"), "{output}");
}

#[test]
fn token_mode_off_loopback_must_name_its_transport() {
    let configuration = GOOD
        .replace("mode = \"trusted-header\"", "mode = \"token\"")
        .replace("127.0.0.1:0", "0.0.0.0:50051");
    let output = refused(&configuration);
    assert!(output.contains("tls-terminated-upstream"), "{output}");
    assert!(
        output.contains("in the clear") || output.contains("shared secret"),
        "{output}"
    );
}

#[test]
fn token_mode_with_no_tokens_points_at_the_mode_that_means_that() {
    let output = refused(&GOOD.replace("mode = \"trusted-header\"", "mode = \"token\""));
    assert!(output.contains("deny-all"), "{output}");
}

#[test]
fn a_short_token_secret_is_refused_with_the_reason() {
    let files = Files::new();
    let secret = files.write("secret", "hunter2");
    let configuration = format!(
        "{}\n[[auth.tokens]]\nname = \"a\"\nsecret_file = \"{}\"\nprincipal = \"u64:1\"\n",
        GOOD.replace("mode = \"trusted-header\"", "mode = \"token\""),
        secret.display()
    );
    let output = refused(&configuration);
    assert!(output.contains("minimum is 32"), "{output}");
    assert!(output.contains("no expiry"), "{output}");
}

#[test]
fn a_secret_cannot_be_written_in_the_configuration_file_at_all() {
    let configuration = format!(
        "{}\n[[auth.tokens]]\nname = \"a\"\nsecret = \"0123456789abcdef0123456789abcdef\"\nprincipal = \"u64:1\"\n",
        GOOD.replace("mode = \"trusted-header\"", "mode = \"token\"")
    );
    // serde refuses the field: there is no `secret` key, on purpose.
    let output = refused(&configuration);
    assert!(output.contains("secret"), "{output}");
}

#[test]
fn a_missing_secret_environment_variable_names_the_variable() {
    let configuration = format!(
        "{}\n[[auth.tokens]]\nname = \"a\"\nsecret_env = \"SLATE_SERVERD_NO_SUCH_VARIABLE\"\nprincipal = \"u64:1\"\n",
        GOOD.replace("mode = \"trusted-header\"", "mode = \"token\"")
    );
    let output = refused(&configuration);
    assert!(
        output.contains("SLATE_SERVERD_NO_SUCH_VARIABLE"),
        "{output}"
    );
}

#[test]
fn an_untagged_principal_is_refused() {
    let files = Files::new();
    let secret = files.write("secret", "0123456789abcdef0123456789abcdef");
    let configuration = format!(
        "{}\n[[auth.tokens]]\nname = \"a\"\nsecret_file = \"{}\"\nprincipal = \"7\"\n",
        GOOD.replace("mode = \"trusted-header\"", "mode = \"token\""),
        secret.display()
    );
    let output = refused(&configuration);
    assert!(output.contains("type tag"), "{output}");
    assert!(output.contains("slate-principal"), "{output}");
}

#[test]
fn a_field_belonging_to_another_mode_is_refused() {
    let configuration = format!(
        "{}\n[[auth.tokens]]\nname = \"a\"\nsecret_env = \"X\"\nprincipal = \"u64:1\"\n",
        GOOD.replace("mode = \"trusted-header\"", "mode = \"deny-all\"")
    );
    let output = refused(&configuration);
    assert!(output.contains("does not use `tokens`"), "{output}");
}

#[test]
fn an_unknown_auth_mode_lists_the_modes() {
    let output = refused(&GOOD.replace("mode = \"trusted-header\"", "mode = \"open\""));
    assert!(
        output.contains("is not a mode this server knows"),
        "{output}"
    );
    assert!(output.contains("deny-all"), "{output}");
}

#[test]
fn a_mistyped_key_under_auth_is_refused_rather_than_ignored() {
    // The failure this prevents: `requre_tenant = true` leaving tenant checks
    // off with no sign anywhere.
    let output = refused(&GOOD.replace(
        "mode = \"trusted-header\"",
        "mode = \"trusted-header\"\nrequre_tenant = true",
    ));
    assert!(output.contains("requre_tenant"), "{output}");
}

// ── the schema ──────────────────────────────────────────────────────────────

#[test]
fn a_predicate_that_does_not_parse_is_shown_with_a_caret() {
    let output = refused(&format!(
        "{GOOD}\n[[tables.indexes]]\nname = \"by_kind\"\nid = 1\ncolumns = [\"kind\"]\nwhere = \"size > 'x'\"\n"
    ));
    assert!(output.contains("table `docs`"), "{output}");
    assert!(output.contains("index `by_kind`"), "{output}");
    assert!(output.contains("size > 'x'"), "{output}");
    assert!(
        output.contains('^'),
        "the error should point at the token: {output}"
    );
    assert!(
        output.contains("cannot be compared with a i64 column"),
        "{output}"
    );
}

#[test]
fn a_predicate_naming_a_column_that_is_not_there_lists_the_ones_that_are() {
    let output = refused(&format!(
        "{GOOD}\n[[tables.checks]]\nname = \"c\"\npredicate = \"sixe > 0\"\n"
    ));
    assert!(output.contains("no column `sixe`"), "{output}");
    assert!(output.contains("`kind`"), "{output}");
}

#[test]
fn a_caller_placeholder_in_a_check_is_refused() {
    let output = refused(&format!(
        "{GOOD}\n[[tables.checks]]\nname = \"c\"\npredicate = \"id = :principal\"\n"
    ));
    assert!(output.contains("security.policies"), "{output}");
}

#[test]
fn an_expression_index_with_no_produces_says_why_it_needs_one() {
    let output = refused(&format!(
        "{GOOD}\n[[tables.indexes]]\nname = \"x\"\nid = 1\nexpression = \"lower(kind)\"\n"
    ));
    assert!(output.contains("needs `produces`"), "{output}");
    assert!(output.contains("before it has one"), "{output}");
}

#[test]
fn two_tables_sharing_an_id_are_refused_by_name() {
    let output = refused(&format!(
        "{GOOD}\n[[tables]]\nname = \"other\"\nid = 1\ncolumns = [{{ name = \"id\", type = \"u64\" }}]\nprimary_key = [\"id\"]\n"
    ));
    assert!(output.contains("share a keyspace"), "{output}");
}

#[test]
fn a_foreign_key_to_a_table_that_is_not_declared_is_refused() {
    let output = refused(&format!(
        "{GOOD}\n[[tables.foreign_keys]]\nname = \"fk\"\nparent = \"nowhere\"\ncolumns = [\"id\"]\n"
    ));
    assert!(output.contains("names no table"), "{output}");
}

#[test]
fn a_tenant_column_that_does_not_lead_the_key_is_refused_by_the_schema_layer() {
    let output = refused(&GOOD.replace(
        "primary_key = [\"id\"]",
        "primary_key = [\"id\"]\ntenant_column = \"kind\"",
    ));
    assert!(output.contains("table `docs`"), "{output}");
}

#[test]
fn a_configuration_with_no_tables_is_refused() {
    let head = GOOD
        .split("[[tables]]")
        .next()
        .expect("there is a prefix")
        .to_owned();
    let output = refused(&head);
    assert!(output.contains("declares no tables"), "{output}");
}

// ── security ────────────────────────────────────────────────────────────────

#[test]
fn a_grant_on_a_table_that_is_not_declared_is_refused() {
    let output = refused(&GOOD.replace("tables = [\"docs\"]", "tables = [\"nowhere\"]"));
    assert!(output.contains("not a table"), "{output}");
}

#[test]
fn an_unknown_action_lists_the_actions() {
    let output = refused(&GOOD.replace("actions = [\"all\"]", "actions = [\"select\"]"));
    assert!(output.contains("read, insert, update, delete"), "{output}");
}

#[test]
fn a_policy_predicate_that_does_not_parse_names_the_policy() {
    let output = refused(&format!(
        "{GOOD}\n[[security.policies]]\nname = \"mine\"\ntable = \"docs\"\nactions = [\"read\"]\nusing = \"id = :nobody\"\n"
    ));
    assert!(output.contains("policy `mine`"), "{output}");
    assert!(output.contains(":principal"), "{output}");
}

#[test]
fn a_policy_with_no_grant_behind_it_is_a_warning_and_not_a_refusal() {
    // Legal, and almost certainly a mistake, so it is said out loud without
    // stopping a node from coming up.
    let output = accepted(&format!(
        "{GOOD}\n[[security.policies]]\nname = \"mine\"\ntable = \"docs\"\nactions = [\"read\"]\nusing = \"size >= 0\"\n"
    ).replace("tables = [\"docs\"]", "tables = []"));
    assert!(output.contains("can never be reached"), "{output}");
}

// ── storage and tuning ──────────────────────────────────────────────────────

#[test]
fn an_unknown_backend_lists_the_backends() {
    // Reached at startup rather than at `--check`, because `--check` does not
    // open storage. Run for real, on a port that will never be bound because
    // the refusal comes first.
    let files = Files::new();
    let path = files.write(
        "head.toml",
        &GOOD.replace("backend = \"memory\"", "backend = \"pg\""),
    );
    let finished = run(&["--config", &path.display().to_string()]);
    assert_eq!(finished.code, Some(2), "{}", finished.output());
    assert!(
        finished.output().contains("`memory`, `local` and `s3`"),
        "{}",
        finished.output()
    );
}

#[test]
fn a_local_backend_with_no_directory_is_refused() {
    let files = Files::new();
    let path = files.write(
        "head.toml",
        &GOOD.replace("backend = \"memory\"", "backend = \"local\""),
    );
    let finished = run(&["--config", &path.display().to_string()]);
    assert_eq!(finished.code, Some(2), "{}", finished.output());
    assert!(
        finished.output().contains("needs `directory`"),
        "{}",
        finished.output()
    );
}

#[test]
fn a_duration_without_a_unit_is_refused() {
    let output = refused(&format!("{GOOD}\n[lease]\nterm = \"15\"\n"));
    assert!(output.contains("no unit"), "{output}");
    assert!(output.contains("lease.term"), "{output}");
}

#[test]
fn a_zero_limit_is_refused_rather_than_silently_useless() {
    let output = refused(&format!("{GOOD}\n[limits]\nrows_per_message = 0\n"));
    assert!(output.contains("send no rows"), "{output}");
}

#[test]
fn an_address_that_is_not_an_address_says_what_one_looks_like() {
    let output = refused(&GOOD.replace("127.0.0.1:0", "localhost"));
    assert!(output.contains("127.0.0.1:50051"), "{output}");
}

// ── the command line ────────────────────────────────────────────────────────

#[test]
fn a_missing_configuration_file_is_reported_by_name() {
    let finished = run(&["--config", "/no/such/file.toml", "--check"]);
    assert_eq!(finished.code, Some(2));
    assert!(
        finished.output().contains("/no/such/file.toml"),
        "{}",
        finished.output()
    );
}

#[test]
fn there_is_no_flag_that_supplies_authentication() {
    // The insecure-by-accident case, stated as a property of the surface: a
    // `--config` with no `[auth]` cannot be started by adding a flag. If one
    // were ever added, `--help` would name it and this would fail.
    let help = run(&["--help"]);
    let text = help.stdout.to_lowercase();
    for forbidden in ["--no-auth", "--insecure", "--allow-anonymous", "--trust"] {
        assert!(
            !text.contains(forbidden),
            "`{forbidden}` exists, and it should not: {text}"
        );
    }
    assert_eq!(help.code, Some(0));
}

#[test]
fn an_unknown_flag_is_refused_and_suggests_something() {
    let files = Files::new();
    let path = files.write("head.toml", GOOD);
    let finished = run(&["--config", &path.display().to_string(), "--porrt", "1"]);
    assert_ne!(finished.code, Some(0));
    assert!(
        finished.output().contains("--porrt") || finished.output().contains("unexpected"),
        "{}",
        finished.output()
    );
}

#[test]
fn print_schema_publishes_ordinals_and_types() {
    let files = Files::new();
    let path = files.write("head.toml", GOOD);
    let finished = run(&["--config", &path.display().to_string(), "--print-schema"]);
    assert_eq!(finished.code, Some(0), "{}", finished.output());
    // The shape a client generator would read: names paired with ordinals.
    assert!(
        finished.stdout.contains("\"ordinal\": 2"),
        "{}",
        finished.stdout
    );
    assert!(finished.stdout.contains("\"size\""), "{}", finished.stdout);
    assert!(
        finished.stdout.contains("\"type\": \"i64\""),
        "{}",
        finished.stdout
    );
}
