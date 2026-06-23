use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use vasari_core::{why_all, Node, ObjectStore};

#[derive(Parser)]
#[command(
    name = "vasari",
    about = "Intent attribution for autonomous coding agents.\n\nLike its namesake — Giorgio Vasari, who invented art attribution by\nasking who painted this, and why — Vasari looks at a line of code\nand answers the same question.",
    version
)]
struct Cli {
    /// Path to the repository root (defaults to current directory).
    #[arg(long, global = true)]
    repo: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Answer: which intent caused this line to exist?
    ///
    /// Example: vasari why src/auth.ts:47
    Why {
        /// File path and line number (e.g., src/auth.ts:47)
        target: String,
        /// Output as newline-delimited JSON (one object per attribution chain)
        #[arg(long)]
        json: bool,
    },
    /// Show where two plans diverged.
    ///
    /// Example: vasari diff plan-a plan-b
    Diff {
        plan_a: String,
        plan_b: String,
    },
    /// Ingest an agent session into the Vasari graph.
    Ingest {
        /// Adapter to use: claude-code | otel-genai
        #[arg(long)]
        adapter: String,
        /// Path to the session file or span export.
        input: PathBuf,
    },
    /// Verify node signatures (opt-in; requires vasari verify setup).
    Verify,
    /// Rebuild indexes and verify object store integrity.
    Fsck,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let repo_root = cli
        .repo
        .unwrap_or_else(|| std::env::current_dir().expect("current dir must be accessible"));

    let store = ObjectStore::open(&repo_root)
        .with_context(|| format!("opening .vasari store at {}", repo_root.display()))?;

    match cli.command {
        Commands::Why { target, json } => cmd_why(&store, &target, json),
        Commands::Diff { plan_a, plan_b } => cmd_diff(&store, &plan_a, &plan_b),
        Commands::Ingest { adapter, input } => cmd_ingest(&store, &adapter, &input),
        Commands::Verify => cmd_verify(),
        Commands::Fsck => cmd_fsck(&store),
    }
}

fn cmd_why(store: &ObjectStore, target: &str, json: bool) -> Result<()> {
    let (path, line) = parse_target(target)?;

    let chains = why_all(store, &path, line)
        .with_context(|| format!("resolving {path}:{line}"))?;

    if chains.is_empty() {
        println!("No attribution found for {path}:{line}");
        println!("Run `vasari ingest` first to populate the graph.");
        return Ok(());
    }

    if json {
        for chain in &chains {
            let obj = serde_json::json!({
                "target": format!("{path}:{line}"),
                "intent_text": chain.primary_intent().map(|i| &i.text),
                "intent_source": chain.primary_intent().map(|i| &i.source),
                "intent_at": chain.primary_intent().map(|i| i.created_at.to_rfc3339()),
                "plan_step_goal": chain.plan_step().map(|s| &s.goal),
                "plan_step_index": chain.plan_step_index + 1,
                "plan_step_total": chain.plan.steps.len(),
                "action_tool": chain.action.tool,
                "confidence": chain.confidence(),
                "attribution_id": chain.attribution.id.as_str(),
            });
            println!("{}", serde_json::to_string(&obj)?);
        }
        return Ok(());
    }

    if chains.len() > 1 {
        println!("{path}:{line} — {} attributions\n", chains.len());
    }

    for (i, chain) in chains.iter().enumerate() {
        if chains.len() > 1 {
            println!("[{}]", i + 1);
        }

        // Lead with the intent — the answer to "why".
        match chain.primary_intent() {
            Some(intent) => {
                println!("{}", intent.text);
                println!("  from  {} · {}", intent.source, intent.created_at.format("%Y-%m-%d %H:%M UTC"));
            }
            None => {
                println!("(orphan plan — no intent recorded)");
            }
        }

        // Plan step context.
        if let Some(step) = chain.plan_step() {
            println!(
                "  via   {} · step {}/{} · {}",
                chain.action.tool,
                chain.plan_step_index + 1,
                chain.plan.steps.len(),
                step.goal,
            );
        } else {
            println!("  via   {}", chain.action.tool);
        }

        // Confidence with evidence kind hint.
        println!("  conf  {:.2}", chain.confidence());

        if chains.len() > 1 {
            println!();
        }
    }

    Ok(())
}

fn cmd_diff(store: &ObjectStore, plan_a_id: &str, plan_b_id: &str) -> Result<()> {
    use vasari_core::schema::NodeId;

    let id_a = NodeId(plan_a_id.to_string());
    let id_b = NodeId(plan_b_id.to_string());

    let Some(Node::Plan(plan_a)) = store.get(&id_a)? else {
        bail!("plan not found: {plan_a_id}");
    };
    let Some(Node::Plan(plan_b)) = store.get(&id_b)? else {
        bail!("plan not found: {plan_b_id}");
    };

    // v0.1 alignment: identical-string or substring match on goal field.
    // v0.2: embedding similarity.
    let max_steps = plan_a.steps.len().max(plan_b.steps.len());
    let mut diverged = false;

    for i in 0..max_steps {
        let step_a = plan_a.steps.get(i);
        let step_b = plan_b.steps.get(i);

        match (step_a, step_b) {
            (Some(a), Some(b)) => {
                let goals_match = a.goal == b.goal
                    || a.goal.contains(&b.goal as &str)
                    || b.goal.contains(&a.goal as &str);
                if !goals_match {
                    if !diverged {
                        println!(
                            "Plans diverge at step {} (1-indexed):",
                            i + 1
                        );
                        diverged = true;
                    }
                    println!("  A step {}: {}", i + 1, a.goal);
                    println!("  B step {}: {}", i + 1, b.goal);
                } else if a.constraints != b.constraints {
                    if !diverged {
                        println!(
                            "Plans diverge at step {} (constraints differ):",
                            i + 1
                        );
                        diverged = true;
                    }
                    println!("  A constraints: {:?}", a.constraints);
                    println!("  B constraints: {:?}", b.constraints);
                }
            }
            (Some(a), None) => {
                if !diverged {
                    println!("Plans diverge at step {} (B is shorter):", i + 1);
                    diverged = true;
                }
                println!("  A step {}: {} (no B counterpart)", i + 1, a.goal);
            }
            (None, Some(b)) => {
                if !diverged {
                    println!("Plans diverge at step {} (A is shorter):", i + 1);
                    diverged = true;
                }
                println!("  B step {}: {} (no A counterpart)", i + 1, b.goal);
            }
            (None, None) => unreachable!(),
        }
    }

    if !diverged {
        println!("Plans are identical across all {} steps.", max_steps);
    }

    Ok(())
}

fn cmd_ingest(store: &ObjectStore, adapter: &str, input: &std::path::Path) -> Result<()> {
    match adapter {
        "claude-code" => {
            println!("Ingesting Claude Code session: {}", input.display());
            println!("(claude-code adapter: not yet implemented — coming in next PR)");
        }
        "otel-genai" => {
            println!("Ingesting OTEL GenAI spans: {}", input.display());
            println!("(otel-genai adapter: not yet implemented — coming in next PR)");
        }
        other => bail!(
            "unknown adapter '{other}'. Available adapters: claude-code, otel-genai"
        ),
    }
    Ok(())
}

fn cmd_verify() -> Result<()> {
    println!("vasari verify: opt-in signing is not yet configured.");
    println!("Run `vasari verify --help` for setup instructions.");
    println!("(Sigstore keyless signing via in-toto DSSE: coming in next PR)");
    Ok(())
}

fn cmd_fsck(store: &ObjectStore) -> Result<()> {
    println!("Rebuilding index from object store…");
    let count = store.rebuild_index()?;
    println!("Done. Indexed {count} attribution nodes.");
    Ok(())
}

/// Parse "src/auth.ts:47" → ("src/auth.ts", 47)
fn parse_target(target: &str) -> Result<(String, u32)> {
    let (path, line_str) = target
        .rsplit_once(':')
        .with_context(|| format!("target must be 'path:line', got '{target}'"))?;
    let line = line_str
        .parse::<u32>()
        .with_context(|| format!("line number must be a positive integer, got '{line_str}'"))?;
    if line == 0 {
        anyhow::bail!("line number must be a positive integer (1-indexed), got '0'");
    }
    Ok((path.to_string(), line))
}
