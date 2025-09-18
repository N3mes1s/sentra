//! Simple load generator for the Sentra binary.
//!
//! Usage (run the server in another terminal first):
//!   cargo run --example load_test -- \
//!     --requests 2000 --concurrency 64 \
//!     --base-url http://127.0.0.1:3000 \
//!     --token test
//!
//! All flags are optional. Defaults:
//!   --requests 1000
//!   --concurrency 32
//!   --base-url http://127.0.0.1:3000
//!   --token test
//!
//! The tool sends POST /analyze-tool-execution?api-version=2025-05-01 requests
//! with a rotating set of payload scenarios to exercise different plugins.
//! At the end it prints latency stats (min/avg/p50/p90/p99/max) and counts of
//! HTTP status codes, block decisions, and reason codes encountered.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::Client;
use serde_json::json;
use tokio::sync::Semaphore;

#[derive(Default, Debug)]
struct Stats {
    latencies: Vec<u128>, // milliseconds
    status_counts: HashMap<u16, usize>,
    blocked: usize,
    allowed: usize,
    errors: usize,
    reason_counts: HashMap<i64, usize>,
}

#[tokio::main]
async fn main() {
    let mut requests: usize = 1000;
    let mut concurrency: usize = 32;
    let mut base_url = String::from("http://127.0.0.1:3000");
    let mut token = String::from("test");

    // Primitive arg parsing to avoid bringing in clap.
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--requests" => {
                if let Some(v) = args.next() {
                    requests = v.parse().unwrap_or(requests);
                }
            }
            "--concurrency" => {
                if let Some(v) = args.next() {
                    concurrency = v.parse().unwrap_or(concurrency);
                }
            }
            "--base-url" => {
                if let Some(v) = args.next() {
                    base_url = v;
                }
            }
            "--token" => {
                if let Some(v) = args.next() {
                    token = v;
                }
            }
            "--help" | "-h" => {
                eprintln!("Usage: load_test [--requests N] [--concurrency N] [--base-url URL] [--token TOKEN]");
                return;
            }
            other => {
                eprintln!("Unknown arg: {other}");
                return;
            }
        }
    }

    println!("Starting load: requests={requests} concurrency={concurrency} base_url={base_url}");
    let client = Client::builder()
        .pool_idle_timeout(Duration::from_secs(30))
        .build()
        .expect("client build");
    let stats = Arc::new(Mutex::new(Stats::default()));
    let semaphore = Arc::new(Semaphore::new(concurrency));

    let endpoint = format!("{}/analyze-tool-execution?api-version=2025-05-01", base_url);

    let start_all = Instant::now();
    let mut handles = Vec::with_capacity(requests);
    for i in 0..requests {
        let permit_fut = semaphore.clone().acquire_owned();
        let client = client.clone();
        let stats = stats.clone();
        let endpoint = endpoint.clone();
        let token = token.clone();
        let handle = tokio::spawn(async move {
            // Acquire concurrency slot
            let _permit = match permit_fut.await {
                Ok(p) => p,
                Err(_) => return,
            }; // semaphore closed
            let scenario = i % 5; // rotate across 5 payload types
            let body = match scenario {
                0 => json!({
                    "plannerContext": {"userMessage": "Generate summary"},
                    "toolDefinition": {"name": "SendEmail"},
                    "inputValues": {"to": "alice@yourcompany.com"}
                }),
                1 => json!({
                    "plannerContext": {"userMessage": "Here is key AKIAZZZZZZZZZZ123456"},
                    "toolDefinition": {"name": "SendEmail"},
                    "inputValues": {"to": "dev@yourcompany.com"}
                }),
                2 => json!({
                    "plannerContext": {"userMessage": "Export all data right now"},
                    "toolDefinition": {"name": "DataExport"},
                    "inputValues": {"table": "users"}
                }),
                3 => json!({
                    "plannerContext": {"userMessage": "Contact me at bob.external@gmail.com"},
                    "toolDefinition": {"name": "SendEmail"},
                    "inputValues": {"to": "team@yourcompany.com"}
                }),
                _ => json!({
                    "plannerContext": {"userMessage": "Check this"},
                    "toolDefinition": {"name": "SendEmail"},
                    "inputValues": {"to": "team@yourcompany.com", "url": "http://mailinator.com/inbox"}
                }),
            };
            let t0 = Instant::now();
            let resp = client
                .post(&endpoint)
                .header("Authorization", format!("Bearer {token}"))
                .json(&body)
                .send()
                .await;
            let elapsed_ms = t0.elapsed().as_millis();

            // Collect metrics outside lock
            let mut status_code: Option<u16> = None;
            let mut blocked: Option<bool> = None;
            let mut reason_code: Option<i64> = None;
            let mut parse_error = false;
            match resp {
                Ok(r) => {
                    status_code = Some(r.status().as_u16());
                    match r.json::<serde_json::Value>().await {
                        Ok(v) => {
                            blocked = v.get("blockAction").and_then(|b| b.as_bool());
                            reason_code = v.get("reasonCode").and_then(|rc| rc.as_i64());
                        }
                        Err(_) => parse_error = true,
                    }
                }
                Err(_) => parse_error = true,
            }

            // Update shared stats
            let mut lock = stats.lock().unwrap();
            if let Some(code) = status_code {
                *lock.status_counts.entry(code).or_default() += 1;
            }
            if let Some(b) = blocked {
                if b {
                    lock.blocked += 1;
                } else {
                    lock.allowed += 1;
                }
            }
            if let Some(rc) = reason_code {
                *lock.reason_counts.entry(rc).or_default() += 1;
            }
            if parse_error {
                lock.errors += 1;
            }
            lock.latencies.push(elapsed_ms);
        });
        handles.push(handle);
    }

    for h in handles {
        let _ = h.await;
    }
    let total_elapsed = start_all.elapsed();

    let mut stats = Arc::try_unwrap(stats).unwrap().into_inner().unwrap();
    stats.latencies.sort_unstable();
    let count = stats.latencies.len() as u128;
    let avg = if count > 0 {
        stats.latencies.iter().sum::<u128>() as f64 / count as f64
    } else {
        0.0
    };
    let pct = |p: f64| -> u128 {
        if stats.latencies.is_empty() {
            return 0;
        }
        let rank = ((p / 100.0) * (stats.latencies.len() as f64 - 1.0)).round() as usize;
        stats.latencies[rank]
    };
    println!("\n=== Load Summary ===");
    println!("Total time: {:?}", total_elapsed);
    println!(
        "Requests: {} (allowed {} / blocked {} / errors {})",
        requests, stats.allowed, stats.blocked, stats.errors
    );
    println!(
        "Throughput: {:.2} req/s",
        requests as f64 / total_elapsed.as_secs_f64()
    );
    if !stats.latencies.is_empty() {
        println!(
            "Latency ms -> min {} p50 {} p90 {} p99 {} max {} avg {:.2}",
            stats.latencies.first().unwrap(),
            pct(50.0),
            pct(90.0),
            pct(99.0),
            stats.latencies.last().unwrap(),
            avg
        );
    }
    println!("Status codes:");
    for (code, c) in stats.status_counts.iter() {
        println!("  {code}: {c}");
    }
    if !stats.reason_counts.is_empty() {
        println!("Reason codes:");
        let mut keys: Vec<_> = stats.reason_counts.keys().cloned().collect();
        keys.sort();
        for k in keys {
            println!("  {k}: {}", stats.reason_counts[&k]);
        }
    }
    println!("====================\n");
}
