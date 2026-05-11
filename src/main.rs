//! `trusty-analyzer` CLI: sidecar daemon + ad-hoc analysis commands.
//!
//! Subcommands:
//! - `serve`        run HTTP daemon (and, with `--mcp`, an MCP stdio loop)
//! - `analyze`      one-shot complexity hotspot report for an index
//! - `facts list|add|delete`
//! - `health`       probe both daemons

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use trusty_analyzer_core::{facts::new_fact, AnalyzerRegistry, FactStore, TrustySearchClient};
use trusty_analyzer_embedder::{BowEmbedder, Embedder, NeuralEmbedder};
use trusty_analyzer_mcp::AnalyzerMcpServer;
use trusty_analyzer_service::{serve, AnalyzerAppState, DEFAULT_PORT};

#[derive(Parser, Debug)]
#[command(
    name = "trusty-analyzer",
    version,
    about = "Sidecar code-analysis daemon for trusty-search"
)]
struct Cli {
    /// Base URL of the trusty-search daemon. Defaults to http://127.0.0.1:7878.
    #[arg(
        long,
        default_value = "http://127.0.0.1:7878",
        env = "TRUSTY_SEARCH_URL"
    )]
    search_url: String,

    /// Path to the redb file backing the analyzer's facts store.
    #[arg(
        long,
        default_value = "trusty-analyzer.facts.redb",
        env = "TRUSTY_ANALYZER_FACTS"
    )]
    facts_path: PathBuf,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Run the HTTP sidecar daemon.
    Serve {
        /// Starting port (auto-detect upward if busy). Defaults to 7879.
        #[arg(long, default_value_t = DEFAULT_PORT)]
        port: u16,
        /// Also run an MCP stdio loop on this process. Useful only when invoked
        /// as a subprocess by an MCP client.
        #[arg(long)]
        mcp: bool,
        /// Start MCP HTTP/SSE server on this port (in addition to or instead of stdio).
        /// Exposes `POST /mcp` and `GET /mcp/sse`.
        #[arg(long)]
        mcp_port: Option<u16>,
        /// Path to the fastembed model cache. If the model is not present
        /// here, the neural embedder fails to load and the daemon falls
        /// back to BOW clustering.
        #[arg(
            long,
            default_value = ".fastembed_cache",
            env = "TRUSTY_FASTEMBED_CACHE"
        )]
        fastembed_cache: PathBuf,
    },
    /// One-shot complexity report for a registered index.
    Analyze {
        index_id: String,
        #[arg(long, default_value_t = 20)]
        top_k: usize,
    },
    /// Facts subcommands.
    Facts {
        #[command(subcommand)]
        op: FactsCmd,
    },
    /// Probe both daemons.
    Health,
    /// Run an MCP stdio server pointed at the analyzer daemon.
    Mcp {
        /// Base URL of the analyzer daemon. Defaults to http://127.0.0.1:7879.
        #[arg(long, default_value = "http://127.0.0.1:7879")]
        analyzer_url: String,
    },
    /// Open the analyzer dashboard UI in the default browser.
    ///
    /// Why: gives users a one-command path to the embedded UI without having
    /// to remember the port or URL. Probes the daemon first so we fail loudly
    /// with a useful message when the daemon isn't running.
    /// What: TCP-probes `127.0.0.1:<port>`, opens `http://127.0.0.1:<port>/ui`
    /// on success, prints a hint on failure.
    /// Test: run with the daemon down — should print the "not running" hint
    /// and exit non-zero. With the daemon up, should open the browser.
    Dashboard {
        /// Port the analyzer daemon is bound to.
        #[arg(long, default_value_t = DEFAULT_PORT, env = "TRUSTY_ANALYZER_PORT")]
        port: u16,
    },
}

#[derive(Subcommand, Debug)]
enum FactsCmd {
    /// List all facts (optionally filtered).
    List {
        #[arg(long)]
        subject: Option<String>,
        #[arg(long)]
        predicate: Option<String>,
        #[arg(long)]
        object: Option<String>,
    },
    /// Add (upsert) a fact.
    Add {
        subject: String,
        predicate: String,
        object: String,
        index_id: String,
    },
    /// Delete a fact by its u64 id.
    Delete { id: u64 },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    let search = TrustySearchClient::new(&cli.search_url);

    match cli.cmd {
        Cmd::Serve {
            port,
            mcp,
            mcp_port,
            fastembed_cache,
        } => {
            // Hard dependency: refuse to start if trusty-search is unreachable.
            // Why: there is no standalone/offline mode — every analysis operation
            // fetches chunk corpora from the search daemon at runtime.
            // What: one GET /health probe before we bind our own port or open redb.
            // Test: run `trusty-analyzer serve` without trusty-search running and
            // verify exit code 1 and the printed error message.
            if !search.health().await.unwrap_or(false) {
                eprintln!(
                    "Error: trusty-search is not reachable at {}\n       Start it first: trusty-search daemon",
                    search.base_url()
                );
                std::process::exit(1);
            }

            let facts = FactStore::open(&cli.facts_path)
                .with_context(|| format!("open facts store at {}", cli.facts_path.display()))?;

            // Try to load the neural embedder. Failure is non-fatal: we fall
            // back to BOW so the daemon still serves clustering requests.
            // Why: keeping the daemon resilient when the ONNX model is
            // missing (CI, fresh machines, offline) is more valuable than
            // hard-failing on startup.
            let embedder: Arc<dyn Embedder> = match NeuralEmbedder::new(Some(&fastembed_cache)) {
                Ok(e) => {
                    tracing::info!("neural embedder loaded from {}", fastembed_cache.display());
                    Arc::new(e)
                }
                Err(e) => {
                    tracing::warn!(
                        "neural embedder failed to load from {} ({e:#}); using BOW",
                        fastembed_cache.display()
                    );
                    Arc::new(BowEmbedder::default())
                }
            };
            let state = AnalyzerAppState::new(search, facts).with_embedder(embedder);

            // Optionally start the MCP HTTP/SSE server on a separate port.
            // Why: some MCP clients (and remote integrations) prefer HTTP/SSE
            // over stdio. Spawned independently of the analyzer's own HTTP
            // daemon so the two ports stay decoupled.
            // What: binds `--mcp-port` and serves `POST /mcp` + `GET /mcp/sse`
            // pointing the dispatcher at `http://127.0.0.1:<port>`.
            // Test: pass `--mcp-port 7880`, then `curl -X POST
            // http://127.0.0.1:7880/mcp -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'`.
            if let Some(mcp_port) = mcp_port {
                let mcp_srv = AnalyzerMcpServer::new(format!("http://127.0.0.1:{port}"));
                let mcp_listener = tokio::net::TcpListener::bind(("127.0.0.1", mcp_port)).await?;
                tracing::info!("MCP HTTP/SSE server listening on port {mcp_port}");
                tokio::spawn(async move {
                    axum::serve(mcp_listener, trusty_analyzer_mcp::sse::router(mcp_srv))
                        .await
                        .ok();
                });
            }

            if mcp {
                // Run both: HTTP daemon in a task, MCP stdio in the foreground.
                let port_for_url = port;
                let http = tokio::spawn(async move {
                    if let Err(e) = serve(state, port).await {
                        tracing::error!("HTTP daemon exited: {e:#}");
                    }
                });
                let mcp_server = AnalyzerMcpServer::new(format!("http://127.0.0.1:{port_for_url}"));
                trusty_analyzer_mcp::stdio::run(mcp_server).await?;
                http.abort();
                Ok(())
            } else {
                serve(state, port).await
            }
        }
        Cmd::Analyze { index_id, top_k } => {
            let chunks = search
                .get_chunks(&index_id)
                .await
                .with_context(|| format!("fetch chunks for {index_id}"))?;
            let report = trusty_analyzer_core::quality::aggregate_quality(&chunks);
            println!(
                "Index: {} | chunks: {} | avg cyclomatic: {:.2} | %A: {:.1}% | smells: {}",
                index_id,
                report.chunk_count,
                report.avg_cyclomatic,
                report.pct_grade_a * 100.0,
                report.smell_count
            );
            // Run the language registry for a per-language structural summary.
            let registry = AnalyzerRegistry::default_registry();
            let static_res = registry.analyze(&chunks);
            println!(
                "\nAnalyzed {} chunks across {} files",
                static_res.analyzed_chunks, static_res.analyzed_files
            );
            // Roll up nodes per language.
            use std::collections::BTreeMap;
            let mut per_lang: BTreeMap<String, (usize, usize)> = BTreeMap::new();
            for n in &static_res.graph.nodes {
                per_lang.entry(n.language.clone()).or_insert((0, 0)).0 += 1;
            }
            for e in &static_res.graph.edges {
                if let Some(n) = static_res.graph.nodes.iter().find(|n| n.id == e.from) {
                    per_lang.entry(n.language.clone()).or_insert((0, 0)).1 += 1;
                }
            }
            for (lang, (nodes, edges)) in &per_lang {
                println!("  {lang}: {nodes} nodes, {edges} edges");
            }

            let hotspots = trusty_analyzer_core::quality::complexity_hotspots(&chunks, top_k);
            println!("\nTop {top_k} complexity hotspots:");
            for (i, c) in hotspots.iter().enumerate() {
                let cyclo =
                    trusty_analyzer_core::complexity::compute_complexity(&c.content).cyclomatic;
                println!(
                    "  {:>3}. cyclo={:>3} {}:{}-{} ({})",
                    i + 1,
                    cyclo,
                    c.file,
                    c.start_line,
                    c.end_line,
                    c.function_name.as_deref().unwrap_or("-")
                );
            }
            Ok(())
        }
        Cmd::Facts { op } => {
            let facts = FactStore::open(&cli.facts_path)?;
            match op {
                FactsCmd::List {
                    subject,
                    predicate,
                    object,
                } => {
                    let hits =
                        facts.query(subject.as_deref(), predicate.as_deref(), object.as_deref())?;
                    println!("{} fact(s)", hits.len());
                    for f in hits {
                        println!(
                            "  [{}] ({}) {} --{}--> {}  prov={:?}",
                            f.id, f.index_id, f.subject, f.predicate, f.object, f.provenance
                        );
                    }
                }
                FactsCmd::Add {
                    subject,
                    predicate,
                    object,
                    index_id,
                } => {
                    let f = new_fact(subject, predicate, object, index_id);
                    let id = f.id;
                    facts.upsert(f)?;
                    println!("upserted: {id}");
                }
                FactsCmd::Delete { id } => {
                    let removed = facts.delete(id)?;
                    println!("removed: {removed}");
                }
            }
            Ok(())
        }
        Cmd::Health => {
            let search_ok = search.health().await.unwrap_or(false);
            println!(
                "trusty-search ({}): {}",
                search.base_url(),
                if search_ok { "OK" } else { "DOWN" }
            );
            // The analyzer's own health is queried via HTTP if it's running.
            let analyzer_url = format!("http://127.0.0.1:{}", DEFAULT_PORT);
            let client = reqwest::Client::new();
            let analyzer_ok = client
                .get(format!("{analyzer_url}/health"))
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            println!(
                "trusty-analyzer ({analyzer_url}): {}",
                if analyzer_ok { "OK" } else { "DOWN" }
            );
            Ok(())
        }
        Cmd::Mcp { analyzer_url } => {
            let server = AnalyzerMcpServer::new(analyzer_url);
            trusty_analyzer_mcp::stdio::run(server).await
        }
        Cmd::Dashboard { port } => {
            use std::net::{SocketAddr, TcpStream};
            use std::time::Duration;
            let addr: SocketAddr = ([127, 0, 0, 1], port).into();
            let reachable = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok();
            if !reachable {
                eprintln!(
                    "Error: trusty-analyzer is not running on port {port}.\n       Start it with: trusty-analyzer serve"
                );
                std::process::exit(1);
            }
            let url = format!("http://127.0.0.1:{port}/ui");
            println!("Opening {url}");
            open::that(&url).with_context(|| format!("open {url} in browser"))?;
            Ok(())
        }
    }
}
