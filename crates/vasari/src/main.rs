use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use vasari_core::{
    ingest::{run_pipeline, IngestAdapter, IngestSource},
    why_all, ConstraintPolarity, Node, NodeId, ObjectStore,
};

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
    Diff { plan_a: String, plan_b: String },
    /// Ingest an agent session into the Vasari graph.
    #[command(subcommand)]
    Ingest(IngestCommands),
    /// Pin a constraint manually (supplements auto-extracted constraints).
    ///
    /// Example: vasari constrain "Never store secrets in env files" --plan <id>
    Constrain {
        /// Constraint text.
        text: String,
        /// Plan node ID this constraint is derived from.
        #[arg(long)]
        plan: String,
        /// Polarity: mandatory (default) or prohibitive.
        #[arg(long, default_value = "mandatory")]
        polarity: String,
    },
    /// List all ingested sessions (Intent nodes).
    Sessions,
    /// List all files with attribution coverage.
    Files,
    /// Verify node signatures (opt-in; requires vasari verify setup).
    Verify,
    /// Rebuild indexes and verify object store integrity.
    Fsck,
}

#[derive(Subcommand)]
enum IngestCommands {
    /// Ingest a Claude Code session JSONL file.
    ///
    /// Example: vasari ingest claude-code ~/.claude/projects/myproject/session.jsonl
    ClaudeCode {
        /// Path to the session .jsonl file, or "-" for stdin.
        input: String,
    },
    /// Ingest an OTLP JSON export with GenAI semantic conventions.
    ///
    /// Requires semconv ≥ 1.30.0 (gen_ai.* attributes).
    /// Example: vasari ingest otel-genai ./spans.json
    OtelGenai {
        /// Path to the OTLP JSON file, or "-" for stdin.
        input: String,
    },
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
        Commands::Ingest(ingest_cmd) => cmd_ingest(&store, ingest_cmd),
        Commands::Constrain {
            text,
            plan,
            polarity,
        } => cmd_constrain(&store, text, plan, polarity),
        Commands::Sessions => cmd_sessions(&store),
        Commands::Files => cmd_files(&store),
        Commands::Verify => cmd_verify(),
        Commands::Fsck => cmd_fsck(&store),
    }
}

fn cmd_why(store: &ObjectStore, target: &str, json: bool) -> Result<()> {
    let (path, line) = parse_target(target)?;

    let chains = why_all(store, &path, line).with_context(|| format!("resolving {path}:{line}"))?;

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
                println!(
                    "  from  {} · {}",
                    intent.source,
                    intent.created_at.format("%Y-%m-%d %H:%M UTC")
                );
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
    let id_a = NodeId(plan_a_id.to_string());
    let id_b = NodeId(plan_b_id.to_string());

    let Some(Node::Plan(plan_a)) = store.get(&id_a)? else {
        bail!("plan not found: {plan_a_id}");
    };
    let Some(Node::Plan(plan_b)) = store.get(&id_b)? else {
        bail!("plan not found: {plan_b_id}");
    };

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
                        println!("Plans diverge at step {} (1-indexed):", i + 1);
                        diverged = true;
                    }
                    println!("  A step {}: {}", i + 1, a.goal);
                    println!("  B step {}: {}", i + 1, b.goal);
                } else if a.constraints != b.constraints {
                    if !diverged {
                        println!("Plans diverge at step {} (constraints differ):", i + 1);
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

fn parse_ingest_source(input: String) -> IngestSource {
    if input == "-" {
        IngestSource::Stdin
    } else {
        IngestSource::File(PathBuf::from(input))
    }
}

fn cmd_ingest(store: &ObjectStore, cmd: IngestCommands) -> Result<()> {
    use vasari_core::adapters::{claude_code::ClaudeCodeAdapter, otel::OtelGenAiAdapter};

    let (adapter_name, events) = match cmd {
        IngestCommands::ClaudeCode { input } => {
            let events = ClaudeCodeAdapter
                .parse(parse_ingest_source(input))
                .with_context(|| "parsing Claude Code session")?;
            ("claude-code", events)
        }
        IngestCommands::OtelGenai { input } => {
            let events = OtelGenAiAdapter
                .parse(parse_ingest_source(input))
                .with_context(|| "parsing OTEL GenAI spans")?;
            ("otel-genai", events)
        }
    };

    let summary = run_pipeline(events, store)
        .with_context(|| format!("running {adapter_name} ingest pipeline"))?;

    println!("Ingest complete ({adapter_name}):");
    println!("  intents:      {}", summary.intents_created);
    println!("  plans:        {}", summary.plans_created);
    println!("  constraints:  {}", summary.constraints_created);
    println!("  actions:      {}", summary.actions_created);
    println!("  attributions: {}", summary.attributions_created);

    if !summary.degraded.is_empty() {
        println!("\nDegraded ({} event(s) skipped):", summary.degraded.len());
        for reason in &summary.degraded {
            println!("  warn: {reason}");
        }
    }

    Ok(())
}

fn cmd_constrain(
    store: &ObjectStore,
    text: String,
    plan_id: String,
    polarity_str: String,
) -> Result<()> {
    use vasari_core::schema::Constraint;

    let polarity = match polarity_str.to_lowercase().as_str() {
        "mandatory" | "m" => ConstraintPolarity::Mandatory,
        "prohibitive" | "p" => ConstraintPolarity::Prohibitive,
        other => bail!("unknown polarity '{other}'. Use: mandatory, prohibitive"),
    };

    let derived_from = NodeId(plan_id.clone());

    // Validate that the plan exists.
    match store.get(&derived_from)? {
        Some(Node::Plan(_)) => {}
        Some(_) => bail!("node {plan_id} exists but is not a Plan"),
        None => bail!("plan not found: {plan_id}"),
    }

    let constraint = Constraint::new(text.clone(), derived_from, polarity, vec![]);
    let id = constraint.id.clone();
    store.put(&Node::Constraint(constraint))?;

    println!("Constraint stored:");
    println!("  id:       {id}");
    println!("  text:     {text}");
    println!("  polarity: {polarity_str}");
    println!("  plan:     {plan_id}");

    Ok(())
}

fn cmd_sessions(store: &ObjectStore) -> Result<()> {
    let nodes = store.iter_all()?;
    let mut intents: Vec<_> = nodes
        .iter()
        .filter_map(|n| {
            if let Node::Intent(i) = n {
                Some(i)
            } else {
                None
            }
        })
        .collect();

    if intents.is_empty() {
        println!("No sessions found. Run `vasari ingest` to populate.");
        return Ok(());
    }

    intents.sort_by_key(|i| i.created_at);
    println!("{} session(s):", intents.len());
    for intent in intents {
        println!(
            "  {} | {} | {}",
            &intent.id.as_str()[..8],
            intent.created_at.format("%Y-%m-%d %H:%M UTC"),
            intent.text.chars().take(60).collect::<String>()
        );
    }

    Ok(())
}

fn cmd_files(store: &ObjectStore) -> Result<()> {
    use std::collections::HashSet;

    let nodes = store.iter_all()?;
    let mut paths: HashSet<String> = HashSet::new();

    for node in &nodes {
        if let Node::Attribution(attr) = node {
            if let vasari_core::schema::AttributionTarget::LineRange { path, .. } = &attr.target {
                paths.insert(path.clone());
            }
        }
    }

    if paths.is_empty() {
        println!("No files with attribution coverage. Run `vasari ingest` first.");
        return Ok(());
    }

    let mut sorted: Vec<_> = paths.into_iter().collect();
    sorted.sort();
    println!("{} file(s) with attribution coverage:", sorted.len());
    for path in sorted {
        println!("  {path}");
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
