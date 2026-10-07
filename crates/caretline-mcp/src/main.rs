//! caretline-mcp: an MCP server (stdio) that lets an agent read and edit caretline editors,
//! live ones a person is using and headless ones it starts itself. See docs/caretline/mcp.md.

mod engine;
mod server;
mod text;
mod tools;

use std::sync::Arc;

use clap::Parser;
use rmcp::ServiceExt;

/// An MCP server for caretline: attach to a live editor and edit alongside the person, with
/// guarded writes, attribution and exact replay. Speaks MCP on stdin and stdout.
#[derive(Parser)]
#[command(
    name = "caretline-mcp",
    version,
    after_help = "Register with Claude Code:\n  claude mcp add caretline -- caretline-mcp\n\nThen start an editor for the agent to join:\n  caretline notes.md --listen\n\nDocs: https://caretline.app/docs/mcp/"
)]
struct Args {
    /// Refuse every edit and save: the agent can only read, watch and trace.
    #[arg(long)]
    read_only: bool,
    /// Accept edits without if_rev (not recommended: an edit may land on text the agent never saw).
    #[arg(long)]
    allow_unguarded: bool,
    /// Don't show "<name>: edited line N" in the live editor's status bar after an edit.
    #[arg(long)]
    quiet: bool,
    /// The name edits are announced under (default: the MCP client's name).
    #[arg(long)]
    name: Option<String>,
    /// Print the tool definitions as JSON and exit.
    #[arg(long)]
    list_tools: bool,
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let args = Args::parse();
    if args.list_tools {
        println!("{}", serde_json::to_string_pretty(&server::tools()).unwrap_or_default());
        return std::process::ExitCode::SUCCESS;
    }
    let config = tools::Config { read_only: args.read_only, allow_unguarded: args.allow_unguarded, announce: !args.quiet, name: args.name };
    let server = server::Server { tools: Arc::new(tools::Tools::new(config)) };
    let running = match server.serve(rmcp::transport::stdio()).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("caretline-mcp: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    if let Err(e) = running.waiting().await {
        eprintln!("caretline-mcp: {e}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}
