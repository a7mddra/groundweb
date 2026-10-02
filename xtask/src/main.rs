//! xtask: repo task runner (`cargo xtask <build|doctor|fmt|dev>`).
//!
//! Alias is configured in `.cargo/config.toml` (`xtask = "run -p xtask --"`).
//! Keep this binary dependency-light and shell out to `cargo`/`rustfmt`/`git`
//! instead of reimplementing them.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::process::Command;

const DEFAULT_MODEL: &str = "openrouter/auto";
const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";
const DEFAULT_PROMPT: &str = "search github for rust openrouter tool calling examples and summarize the best approaches with links";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("build") => cmd_build(&args[2..]),
        Some("doctor") => cmd_doctor(&args[2..]),
        Some("fmt") => cmd_fmt(&args[2..]),
        Some("dev") => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("build tokio runtime")?;
            rt.block_on(cmd_dev(&args[2..]))
        }
        Some("--help") | Some("-h") | None => {
            eprintln!("usage: cargo xtask <build|doctor|fmt|dev> [flags]");
            eprintln!("  build [--release]");
            eprintln!("  doctor");
            eprintln!("  fmt [--all]        (default: git-changed *.rs only)");
            eprintln!("  dev [--prompt ...] [--model ...] [--base-url ...] [--max-iters N] [--live] [--max-results N]");
            if args.get(1).is_none() {
                bail!("no xtask given");
            }
            Ok(())
        }
        Some(other) => {
            bail!("unknown xtask '{other}'. expected: build | doctor | fmt | dev");
        }
    }
}

// ---- build ----

fn cmd_build(flags: &[String]) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.arg("build").arg("--workspace");
    if flags.iter().any(|f| f == "--release") {
        cmd.arg("--release");
    }
    println!("> {:?}", cmd);
    let status = cmd.status().context("run cargo build --workspace")?;
    if !status.success() {
        bail!("cargo build failed");
    }
    Ok(())
}

// ---- doctor ----

fn cmd_doctor(_flags: &[String]) -> Result<()> {
    let mut failed = false;
    let mut check = |label: &str, ok: bool, detail: String| {
        let mark = if ok { "ok  " } else { "FAIL" };
        println!("[{mark}] {label}: {detail}");
        if !ok {
            failed = true;
        }
    };

    let rustc = Command::new("rustc").arg("--version").output();
    check(
        "rustc",
        rustc.is_ok(),
        rustc
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|e| e.to_string()),
    );

    for path in [
        "Cargo.toml",
        "opensearch-rs/Cargo.toml",
        "opensearch-rs/src/lib.rs",
        "xtask/Cargo.toml",
        "xtask/src/main.rs",
        ".cargo/config.toml",
        "rust-toolchain.toml",
        ".env.example",
        ".gitignore",
    ] {
        check(
            path,
            std::path::Path::new(path).exists(),
            if std::path::Path::new(path).exists() {
                "present".into()
            } else {
                "missing".into()
            },
        );
    }

    // .env / key is a warning, not a hard failure (build/doctor must work offline).
    let _ = dotenvy::dotenv();
    match std::env::var("OPENROUTER_API_KEY") {
        Ok(k) if !k.is_empty() && k != "sk-or-..." => {
            println!("[ok  ] OPENROUTER_API_KEY: set (.env or env)");
        }
        _ => println!("[warn] OPENROUTER_API_KEY: missing — `cargo xtask dev` will fail until you copy .env.example to .env"),
    }

    if failed {
        bail!("doctor found missing files/toolchain (see FAIL lines)");
    }
    println!("doctor: workspace layout looks good");
    Ok(())
}

// ---- fmt ----

fn cmd_fmt(flags: &[String]) -> Result<()> {
    if flags.iter().any(|f| f == "--all") {
        println!("> cargo fmt --all");
        let status = Command::new("cargo")
            .args(["fmt", "--all"])
            .status()
            .context("run cargo fmt --all")?;
        if !status.success() {
            bail!("cargo fmt --all failed");
        }
        return Ok(());
    }

    // Default: only format git-changed / untracked *.rs files.
    let inside = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !inside {
        bail!("not a git repo; use `cargo xtask fmt --all` instead");
    }
    let tracked = Command::new("git")
        .args(["diff", "--name-only", "HEAD"])
        .output()
        .context("git diff")?;
    let untracked = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .output()
        .context("git ls-files")?;
    let mut files: Vec<String> = String::from_utf8_lossy(&tracked.stdout)
        .lines()
        .chain(String::from_utf8_lossy(&untracked.stdout).lines())
        .map(str::trim)
        .filter(|f| f.ends_with(".rs"))
        .map(str::to_string)
        .collect();
    files.sort();
    files.dedup();
    // Drop deleted paths.
    files.retain(|f| std::path::Path::new(f).exists());

    if files.is_empty() {
        println!("fmt: no changed rust files (use --all for whole repo)");
        return Ok(());
    }
    println!("> rustfmt --edition 2021 -- {}", files.join(" "));
    let status = Command::new("rustfmt")
        .arg("--edition")
        .arg("2021")
        .arg("--")
        .args(&files)
        .status()
        .context("run rustfmt")?;
    if !status.success() {
        bail!("rustfmt failed");
    }
    Ok(())
}

// ---- dev ----

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<serde_json::Value>,
    tools: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: RespMessage,
}

#[derive(Debug, Deserialize)]
struct RespMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Deserialize)]
struct ToolCall {
    id: String,
    function: ToolFn,
}

#[derive(Debug, Deserialize)]
struct ToolFn {
    name: String,
    arguments: String,
}

/// Keyless live run: real `opensearch::execute` (Mojeek branch, safe-source
/// fallback). No OpenRouter key needed — Mojeek is free.
async fn cmd_dev_live(prompt: &str, max_results: Option<usize>) -> Result<()> {
    let args = opensearch::SearchArgs {
        query: prompt.to_string(),
        urls: vec![],
        branch: opensearch::SearchBranch::Mojeek,
        max_results,
    };
    println!("== opensearch dev --live ==");
    println!("branch:  mojeek (+ safe_fallback), keyless");
    println!("query:   {}", args.query);
    println!();

    let out = opensearch::execute(args)
        .await
        .map_err(|e| anyhow::anyhow!("live search failed: {e}"))?;

    println!("mode:    {}", out.mode);
    println!("answer:  {}", out.answer_stub);
    println!("sources: {}", out.sources.len());
    for (i, s) in out.sources.iter().enumerate() {
        println!("--- source {} ---", i + 1);
        println!("title:   {}", s.title);
        println!("url:     {}", s.url);
        println!("summary: {}", s.summary);
        println!(
            "favicon: {} / inlined: {}",
            s.favicon_url.as_deref().unwrap_or("-"),
            if s.favicon_base64.is_some() {
                "yes"
            } else {
                "no"
            }
        );
    }
    println!("[thinking]");
    for t in &out.grounded.thinking {
        println!("- {t}");
    }
    let ctx = out.context_markdown.chars().take(3000).collect::<String>();
    println!("[context head]\n{ctx}");
    Ok(())
}

async fn cmd_dev(flags: &[String]) -> Result<()> {
    let _ = dotenvy::dotenv();
    let mut prompt = None::<String>;
    let mut model = None::<String>;
    let mut base_url = None::<String>;
    let mut max_iters = 6usize;
    let mut live = false;
    let mut max_results = None::<usize>;

    let mut i = 0;
    while i < flags.len() {
        match flags[i].as_str() {
            "--prompt" => {
                i += 1;
                prompt = Some(flags.get(i).cloned().unwrap_or_default());
            }
            s if s.starts_with("--prompt=") => {
                prompt = Some(s["--prompt=".len()..].to_string());
            }
            "--model" => {
                i += 1;
                model = Some(flags.get(i).cloned().unwrap_or_default());
            }
            s if s.starts_with("--model=") => {
                model = Some(s["--model=".len()..].to_string());
            }
            "--base-url" => {
                i += 1;
                base_url = Some(flags.get(i).cloned().unwrap_or_default());
            }
            "--max-iters" => {
                i += 1;
                max_iters = flags.get(i).and_then(|v| v.parse().ok()).unwrap_or(6);
            }
            "--live" => {
                live = true;
            }
            "--max-results" => {
                i += 1;
                max_results = flags.get(i).and_then(|v| v.parse().ok());
            }
            "--help" | "-h" => {
                println!("usage: cargo xtask dev [--prompt ...] [--model ...] [--base-url ...] [--max-iters N] [--live] [--max-results N]");
                println!("  default: OpenRouter tool-call loop with the placeholder tool (needs OPENROUTER_API_KEY).");
                println!("  --live:  run the real Mojeek branch keylessly via opensearch::execute (no API key).");
                println!("presets to try: \"search github for <topic> ...\", \"search <xxxx> and compare ...\"");
                return Ok(());
            }
            other => bail!("unknown dev flag '{other}' (see --help)"),
        }
        i += 1;
    }

    let prompt = prompt.unwrap_or_else(|| DEFAULT_PROMPT.to_string());

    if live {
        return cmd_dev_live(&prompt, max_results).await;
    }

    let api_key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
    if api_key.is_empty() || api_key == "sk-or-..." {
        bail!("OPENROUTER_API_KEY missing: copy .env.example to .env and set a real key");
    }
    let model = model
        .or_else(|| std::env::var("OPENROUTER_MODEL").ok())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let base_url = base_url
        .or_else(|| std::env::var("OPENROUTER_BASE_URL").ok())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

    println!("== opensearch dev ==");
    println!("model:   {model}");
    println!("prompt:  {prompt}");
    println!(
        "tool:    {} (placeholder — no real search, proves tool-call loop)",
        opensearch::TOOL_NAME
    );
    println!();

    let client = reqwest::Client::new();
    let mut messages: Vec<serde_json::Value> = vec![
        serde_json::json!({
            "role": "system",
            "content": "You have a `web_search` tool. If the user prompt needs fresh or external facts (e.g. starts with or contains 'search ...'), you MUST call it before answering. After tool results arrive, write the final response BASED on those results and cite the grounded urls. Keep intermediate thinking brief."
        }),
        serde_json::json!({ "role": "user", "content": prompt }),
    ];

    for iter in 1..=max_iters {
        let req = ChatRequest {
            model: model.clone(),
            messages: messages.clone(),
            tools: vec![opensearch::tool_definition()],
            tool_choice: Some("auto".into()),
        };
        let resp = client
            .post(format!(
                "{}/chat/completions",
                base_url.trim_end_matches('/')
            ))
            .header("Authorization", format!("Bearer {api_key}"))
            .header("HTTP-Referer", "https://github.com/opensearch-rs")
            .header("X-Title", "opensearch-xtask-dev")
            .json(&req)
            .send()
            .await
            .context("POST openrouter /chat/completions")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("openrouter {status}: {body}");
        }
        let chat: ChatResponse = resp.json().await.context("decode chat response")?;
        let msg = chat.choices.first().context("empty choices")?;

        println!("--- iter {iter} ---");
        if let Some(c) = &msg.message.content {
            if !c.trim().is_empty() {
                println!("[model text]\n{c}\n");
            }
        }

        if msg.message.tool_calls.is_empty() {
            println!("== final (no further tool calls) ==");
            return Ok(());
        }

        for tc in &msg.message.tool_calls {
            println!(
                "[tool call] {} args={}",
                tc.function.name, tc.function.arguments
            );
            let result_str = if tc.function.name == opensearch::TOOL_NAME {
                let args_json: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                    .unwrap_or_else(
                        |_| serde_json::json!({"query": tc.function.arguments.clone()}),
                    );
                match opensearch::SearchArgs::from_json(&args_json) {
                    Ok(args) => {
                        let out = opensearch::SearchOutput::placeholder(&args);
                        println!(
                            "[grounded] urls_fetched={} briefs={} thinking={}",
                            out.grounded.urls_fetched.len(),
                            out.grounded.briefs.len(),
                            out.grounded.thinking.len()
                        );
                        serde_json::to_string(&out)
                            .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
                    }
                    Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
                }
            } else {
                serde_json::json!({"error": format!("unknown tool {}", tc.function.name)})
                    .to_string()
            };

            // Append assistant tool_call + tool result so the loop continues.
            messages.push(serde_json::json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": tc.id,
                    "type": "function",
                    "function": { "name": tc.function.name, "arguments": tc.function.arguments }
                }]
            }));
            messages.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": tc.id,
                "content": result_str
            }));
        }
    }

    bail!("max-iters ({max_iters}) hit without a final answer — retry with a narrower prompt");
}
