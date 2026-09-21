//! `agentctl` — operator CLI for a local `agentd`.
//!
//! Usage: `agentctl [--socket PATH] <command> [args]`
//! The socket defaults to `$AGENTOS_SOCKET`, then `~/.agentos/agentd.sock`.
//! Output is always a single JSON document on stdout; failures go to
//! stderr as `{"error": ...}` with a non-zero exit code.

use std::path::PathBuf;

fn socket_path(args: &[String]) -> (PathBuf, Vec<String>) {
    let mut socket = std::env::var_os("AGENTOS_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".agentos")
                .join("agentd.sock")
        });
    let mut rest = Vec::new();
    let mut it = args.iter().peekable();
    while let Some(arg) = it.next() {
        if let Some(v) = arg.strip_prefix("--socket=") {
            socket = PathBuf::from(v);
        } else if arg == "--socket" {
            if let Some(v) = it.next() {
                socket = PathBuf::from(v);
            }
        } else {
            rest.push(arg.clone());
        }
    }
    (socket, rest)
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (socket, rest) = socket_path(&args);
    if rest.is_empty() || rest[0] == "help" || rest[0] == "--help" {
        println!(
            "agentctl [--socket PATH] <command>\n\
             \n\
             commands:\n\
             \x20 health\n\
             \x20 create-session [--metadata FILE|@FILE|-]\n\
             \x20 put-agent-spec [id] --version V --body FILE\n\
             \x20 create-run --session-id S --task-kind K [--payload FILE] [--agent-spec-id X --spec-version V] [--parent-run-id R] [--profile P]\n\
             \x20 get-run RUN_ID\n\
             \x20 graph TASK_ID\n\
             \x20 cancel-run RUN_ID [--expected-revision N] [--reason R]\n\
             \x20 get-effect EFFECT_ID\n\
             \x20 resolve-effect --effect-id E --action A [--expected-state N] [--result-ref R] [--reason R] [--approval-request-id A]\n\
             \x20 adapters [--port-id P]\n\
             \x20 config show\n\
             \x20 config propose --file F\n\
             \x20 config test --generation-id G [--digest D]\n\
             \x20 config activate --generation-id G [--expected-revision N]\n\
             \x20 config rollback --generation-id G [--expected-revision N] [--reason R]\n\
             \x20 approvals list [--run-id R]\n\
             \x20 approvals respond --request-id X --digest D --decision approve|deny [--device-id D]\n\
             \x20 events read --stream-key K [--from N] [--limit N]\n\
             \x20 events tail --stream-key K [--after N] [--count N] [--timeout S]\n\
             \x20 replay RUN_ID\n"
        );
        return std::process::ExitCode::SUCCESS;
    }
    match agentctl::run(&rest, &socket).await {
        Ok(value) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&value).unwrap_or_default()
            );
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{}", serde_json::json!({ "error": e.to_string() }));
            std::process::ExitCode::FAILURE
        }
    }
}
